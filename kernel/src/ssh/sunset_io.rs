//! Bridge between `sunset::Runner` and our `net::sockets` async helpers.
//!
//! Polled single-task pattern (the kernel is cooperative + single-CPU):
//! every iteration we
//!   1. drain `runner.output_buf()` to the socket (non-blocking),
//!   2. push fresh socket bytes into `runner.input()`,
//!   3. drive `runner.progress()` to consume one event,
//!   4. shuffle channel data between the SSH channel and the attached
//!      PTY pair (Task 8 — phase 2 interactive shell).
//! When nothing made progress this iteration, we yield 1 tick.

use alloc::boxed::Box;
use alloc::string::ToString;
use smoltcp::iface::SocketHandle;
use sunset::{
    event::{Event, ServEvent},
    ChanData, ChanHandle, Runner, SignKey,
};

use crate::executor::delay::Delay;
use crate::net::sockets;
use crate::ssh::SshError;

/// SSH max packet sizing — matches the picow demo. SSH frames are usually
/// well under 2 KiB; this is plenty of headroom for the kernel.
const SSH_BUF: usize = 4096;
const SOCK_CHUNK: usize = 1024;

/// Run one SSH session to completion.
///
/// `host_signkey` is the server's Ed25519 signing key; `authkeys` is the
/// set of authorised client pubkeys (raw 32-byte ed25519). Returns once the
/// session is `Defunct` or the socket closes.
pub async fn run_session(
    handle: SocketHandle,
    host_signkey: SignKey,
    authkeys: alloc::vec::Vec<[u8; 32]>,
) -> Result<(), SshError> {
    let mut inbuf:  Box<[u8]> = alloc::vec![0u8; SSH_BUF].into_boxed_slice();
    let mut outbuf: Box<[u8]> = alloc::vec![0u8; SSH_BUF].into_boxed_slice();
    let mut runner = Runner::new_server(&mut inbuf[..], &mut outbuf[..]);

    let mut rxbuf = [0u8; SOCK_CHUNK];
    // Pending input bytes that runner.input() refused (e.g. before
    // initial_sent). Re-tried at the top of each iteration before
    // pulling more from the socket. Without this, the client banner
    // (sent before the server's initial_sent flag flips) is silently
    // dropped on iter=1 — we'd then never make protocol progress.
    let mut pending_in: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let mut chan: Option<ChanHandle> = None;
    let mut pty_idx: Option<usize> = None;
    let mut auth_ok = false;
    let mut shell_started = false;
    let mut iter_count: u64 = 0;

    loop {
        iter_count += 1;
        let mut progressed = false;
        if iter_count <= 10 || iter_count % 50 == 0 {
            crate::binfo!(
                "ssh", "iter={} outbuf={} authok={} chan={}",
                iter_count, runner.output_buf().len(), auth_ok, chan.is_some()
            );
        }

        // 1) Drain runner.output_buf -> socket.
        let out_len = runner.output_buf().len();
        if out_len > 0 {
            // Snapshot bytes so we can drop the borrow before consume_output.
            let mut chunk = [0u8; SOCK_CHUNK];
            let n = out_len.min(SOCK_CHUNK);
            chunk[..n].copy_from_slice(&runner.output_buf()[..n]);
            match sockets::try_send(handle, &chunk[..n]) {
                Some(0) => {
                    crate::bwarn!("ssh", "iter={} try_send returned 0 (closed)", iter_count);
                    runner.close_output();
                    break;
                }
                Some(sent) => {
                    if iter_count <= 10 {
                        crate::binfo!("ssh", "iter={} sent {}/{} bytes", iter_count, sent, n);
                    }
                    runner.consume_output(sent);
                    progressed = true;
                }
                None => {
                    if iter_count <= 10 {
                        crate::binfo!("ssh", "iter={} try_send None (can't yet)", iter_count);
                    }
                }
            }
        }

        // 2a) Retry pending input first (refused by runner.input earlier).
        if !pending_in.is_empty() {
            let mut consumed = 0;
            while consumed < pending_in.len() {
                match runner.input(&pending_in[consumed..]) {
                    Ok(c) if c > 0 => { consumed += c; progressed = true; }
                    _              => break,
                }
            }
            if consumed > 0 {
                pending_in.drain(0..consumed);
            }
        }

        // 2b) Pull fresh socket bytes -> runner.input. Anything refused is
        //     stashed in pending_in for the next iteration.
        if pending_in.is_empty() {
            match sockets::try_recv(handle, &mut rxbuf) {
                Some(0) => { runner.close_input(); }
                Some(n) => {
                    let mut slice = &rxbuf[..n];
                    while !slice.is_empty() {
                        match runner.input(slice) {
                            Ok(c) if c > 0 => { slice = &slice[c..]; progressed = true; }
                            _              => break,
                        }
                    }
                    if !slice.is_empty() {
                        pending_in.extend_from_slice(slice);
                    }
                }
                None => {}
            }
        }

        // 3) Drive one progress event. Each match arm consumes the event
        //    guard before the next progress() call. We wrap the match in
        //    a labelled block so the &mut runner borrow is released before
        //    the channel-I/O section below.
        let session_done = 'evt: {
        if iter_count <= 3 { crate::binfo!("ssh", "iter={} pre-progress", iter_count); }
        let pr = runner.progress();
        if iter_count <= 3 { crate::binfo!("ssh", "iter={} post-progress", iter_count); }
        let ev_dbg = match &pr {
            Ok(Event::Serv(ServEvent::Hostkeys(_)))    => "Hostkeys",
            Ok(Event::Serv(ServEvent::FirstAuth(_)))   => "FirstAuth",
            Ok(Event::Serv(ServEvent::PasswordAuth(_))) => "PasswordAuth",
            Ok(Event::Serv(ServEvent::PubkeyAuth(_)))  => "PubkeyAuth",
            Ok(Event::Serv(ServEvent::OpenSession(_))) => "OpenSession",
            Ok(Event::Serv(ServEvent::SessionPty(_)))  => "SessionPty",
            Ok(Event::Serv(ServEvent::SessionEnv(_)))  => "SessionEnv",
            Ok(Event::Serv(ServEvent::SessionShell(_))) => "SessionShell",
            Ok(Event::Serv(ServEvent::SessionExec(_))) => "SessionExec",
            Ok(Event::Serv(ServEvent::SessionSubsystem(_))) => "SessionSubsystem",
            Ok(Event::Serv(ServEvent::Defunct))        => "Defunct",
            Ok(_)                                       => "OtherOk",
            Err(_)                                      => "Err",
        };
        if iter_count <= 20 || (!matches!(ev_dbg, "OtherOk")) {
            crate::binfo!("ssh", "iter={} ev={}", iter_count, ev_dbg);
        }
        match pr {
            Ok(Event::Serv(ServEvent::Hostkeys(h))) => {
                let _ = h.hostkeys(&[&host_signkey]);
                progressed = true;
            }
            Ok(Event::Serv(ServEvent::FirstAuth(mut a))) => {
                let _ = a.enable_password_auth(false);
                let _ = a.enable_pubkey_auth(true);
                progressed = true;
            }
            Ok(Event::Serv(ServEvent::PasswordAuth(a))) => {
                let _ = a.reject();
                progressed = true;
            }
            Ok(Event::Serv(ServEvent::PubkeyAuth(a))) => {
                let real = a.real();
                let user = a.username().ok().map(|s| s.to_string()).unwrap_or_default();
                let key_bytes: Option<[u8; 32]> = match a.pubkey() {
                    Ok(sunset::PubKey::Ed25519(epk)) => Some(epk.key.0),
                    _ => None,
                };
                crate::binfo!(
                    "ssh", "PubkeyAuth user={} real={} have_key={} keys_total={}",
                    user, real, key_bytes.is_some(), authkeys.len()
                );
                if let Some(kb) = key_bytes {
                    let matched = authkeys.iter().any(|k| k == &kb);
                    crate::binfo!("ssh", "key match={} first8={:02x?}", matched, &kb[..8]);
                    if matched {
                        if real {
                            crate::binfo!("ssh", "auth ok user={} (real sig)", user);
                            auth_ok = true;
                        }
                        let _ = a.allow();
                    } else {
                        crate::bwarn!("ssh", "auth reject user={} (unknown key)", user);
                        let _ = a.reject();
                    }
                } else {
                    crate::bwarn!("ssh", "auth reject user={} (no ed25519 key)", user);
                    let _ = a.reject();
                }
                progressed = true;
            }
            Ok(Event::Serv(ServEvent::OpenSession(s))) => {
                if !auth_ok {
                    let _ = s.reject(sunset::ChanFail::SSH_OPEN_ADMINISTRATIVELY_PROHIBITED);
                } else {
                    match s.accept() {
                        Ok(h) => {
                            crate::binfo!("ssh", "channel opened");
                            chan = Some(h);
                        }
                        Err(_) => crate::bwarn!("ssh", "channel accept failed"),
                    }
                }
                progressed = true;
            }
            Ok(Event::Serv(ServEvent::SessionPty(p))) => {
                let _ = p.succeed();
                progressed = true;
            }
            Ok(Event::Serv(ServEvent::SessionEnv(e))) => {
                let _ = e.succeed();
                progressed = true;
            }
            Ok(Event::Serv(ServEvent::SessionShell(s))) => {
                if chan.is_some() && !shell_started {
                    // Allocate a PTY pair + spawn shell.wasm on its slave.
                    if let Some(idx) = alloc_and_spawn_shell() {
                        pty_idx = Some(idx);
                        shell_started = true;
                        let _ = s.succeed();
                        crate::binfo!("ssh", "shell started on pty {}", idx);
                    } else {
                        let _ = s.fail();
                        crate::bwarn!("ssh", "no free pty for shell");
                    }
                } else {
                    let _ = s.fail();
                }
                progressed = true;
            }
            Ok(Event::Serv(ServEvent::SessionExec(e))) => {
                // Phase-1 exec is deferred — non-PTY stdout teeing not wired.
                let _ = e.fail();
                progressed = true;
            }
            Ok(Event::Serv(ServEvent::SessionSubsystem(s))) => {
                let _ = s.fail();
                progressed = true;
            }
            Ok(Event::Serv(ServEvent::Defunct)) => break 'evt true,
            Ok(_) => {} // Authenticated / PollAgain etc — keep looping
            Err(e) => {
                crate::bwarn!("ssh", "progress error: {:?}", e);
                break 'evt true;
            }
        }
        false
        };
        if session_done { break; }

        // 4) Bridge channel ↔ PTY (Task 8).
        if let (Some(c), Some(idx)) = (chan.as_ref(), pty_idx) {
            // SSH -> PTY master input (what the user typed).
            let mut ch_in = [0u8; 64];
            match runner.read_channel(c, ChanData::Normal, &mut ch_in) {
                Ok(n) if n > 0 => {
                    for &b in &ch_in[..n] { crate::pty::master_input_push(idx, b); }
                    progressed = true;
                }
                _ => {}
            }
            // PTY master output (what shell.wasm printed) -> SSH channel.
            let mut tx_buf = [0u8; 64];
            let mut filled = 0usize;
            while filled < tx_buf.len() {
                if let Some(b) = crate::pty::master_output_try(idx) {
                    tx_buf[filled] = b; filled += 1;
                } else {
                    break;
                }
            }
            if filled > 0 {
                if let Ok(w) = runner.write_channel(c, ChanData::Normal, &tx_buf[..filled]) {
                    if w < filled {
                        // Bytes lost — TODO: buffer locally. For MVP we
                        // accept best-effort; the channel window should
                        // refill on next pass.
                        let _ = w;
                    }
                    progressed = true;
                }
            }
        }

        if !progressed {
            Delay::ticks(1).await;
        }
    }
    // Tidy up: discard channel + close socket.
    if let Some(c) = chan {
        let _ = runner.channel_done(c);
    }
    sockets::close(handle);
    crate::binfo!("ssh", "session done");
    Ok(())
}

/// Allocate one of the PTY pairs and spawn `shell.wasm` on its slave.
/// Returns the pair index on success.
fn alloc_and_spawn_shell() -> Option<usize> {
    // Pair 0 is owned by the framebuffer console (boot shell). Pairs 1..4 are
    // free for SSH-driven shells.
    for idx in 1..crate::pty::NUM_PAIRS {
        if crate::pty::try_claim(idx) {
            // Spawn shell.wasm with stdin/stdout/stderr bound to pts/idx via
            // the existing wasm_task pattern. We post into exec_queue with
            // an explicit PTY index.
            crate::wasm::ssh_spawn::spawn_shell_on_pty(idx);
            return Some(idx);
        }
    }
    None
}
