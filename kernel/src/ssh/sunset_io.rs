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
    passwd:   Option<alloc::sync::Arc<crate::ssh::password::PasswordCheck>>,
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
    // Pending output bytes pulled from the PTY but not yet accepted by
    // `runner.write_channel` (e.g. when the SSH channel's send window is
    // momentarily exhausted under bridged VirtualBox networking, where
    // throughput/latency differ from QEMU SLIRP). Retried each iteration
    // before pulling fresh bytes from the PTY master output queue. Without
    // this, the bridge silently drops shell stdout under back-pressure —
    // observed symptom: typed commands run but the client sees no output.
    let mut pending_out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let mut chan: Option<ChanHandle> = None;
    let mut pty_idx: Option<usize> = None;
    let mut auth_ok = false;
    let mut shell_started = false;
    // Set once the spawned shell/command has exited and we've sent the
    // channel EOF+CLOSE; the loop then drains the socket and ends.
    let mut closing = false;
    // Byte counters for the channel<->PTY bridge, logged at session end.
    let mut rx_total: u64 = 0; // bytes SSH channel -> PTY (client input)
    let mut tx_total: u64 = 0; // bytes PTY -> SSH channel (shell output)
    let mut tx_dropped: u64 = 0; // bytes pulled from PTY but write_channel refused
    let mut sent_total: u64 = 0; // bytes actually handed to smoltcp via try_send

    loop {
        let mut progressed = false;

        // 1) Drain runner.output_buf -> socket.
        let out_len = runner.output_buf().len();
        if out_len > 0 {
            // Snapshot bytes so we can drop the borrow before consume_output.
            let mut chunk = [0u8; SOCK_CHUNK];
            let n = out_len.min(SOCK_CHUNK);
            chunk[..n].copy_from_slice(&runner.output_buf()[..n]);
            match sockets::try_send(handle, &chunk[..n]) {
                Some(0) => {
                    crate::bwarn!("ssh", "try_send returned 0 (socket closed)");
                    runner.close_output();
                    break;
                }
                Some(sent) => {
                    sent_total += sent as u64;
                    runner.consume_output(sent);
                    progressed = true;
                }
                None => {} // socket TX buffer full; retry next iteration
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
        let pr = runner.progress();
        match pr {
            Ok(Event::Serv(ServEvent::Hostkeys(h))) => {
                let _ = h.hostkeys(&[&host_signkey]);
                progressed = true;
            }
            Ok(Event::Serv(ServEvent::FirstAuth(mut a))) => {
                // Pubkey is always offered. Password offered only when a
                // valid /mnt/passwd was parsed — otherwise the option
                // shouldn't appear in the client's auth method list.
                let _ = a.enable_password_auth(passwd.is_some());
                let _ = a.enable_pubkey_auth(true);
                progressed = true;
            }
            Ok(Event::Serv(ServEvent::PasswordAuth(a))) => {
                let user = a.username().ok().map(|s| s.to_string()).unwrap_or_default();
                let pw_ok = match (passwd.as_deref(), a.password()) {
                    (Some(h), Ok(pw)) => crate::ssh::password::verify(pw, h),
                    _                  => false,
                };
                if pw_ok {
                    crate::binfo!("ssh", "auth ok user={} (password)", user);
                    auth_ok = true;
                    let _ = a.allow();
                } else {
                    crate::bwarn!("ssh", "auth reject user={} (bad password)", user);
                    let _ = a.reject();
                }
                progressed = true;
            }
            Ok(Event::Serv(ServEvent::PubkeyAuth(a))) => {
                let real = a.real();
                let user = a.username().ok().map(|s| s.to_string()).unwrap_or_default();
                let key_bytes: Option<[u8; 32]> = match a.pubkey() {
                    Ok(sunset::PubKey::Ed25519(epk)) => Some(epk.key.0),
                    _ => None,
                };
                if let Some(kb) = key_bytes {
                    let matched = authkeys.iter().any(|k| k == &kb);
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
                // Run the command by spawning shell.wasm on a PTY and feeding
                // "<cmd>\nexit\n" to its input. Output streams back over the
                // channel via the bridge below. Same mechanism as the
                // interactive shell, just pre-seeded with the command.
                //
                // KNOWN LIMITATION (sunset 0.4.0): a non-interactive client
                // (`ssh host cmd`, or any piped stdin) closes its stdin
                // immediately, sending CHANNEL_EOF. sunset's `handle_eof`
                // (channel.rs) auto-mirrors EOF back on the server's output
                // half before the command has produced output, so the client
                // closes its read side and truncates the result. Interactive
                // sessions (stdin stays open) are unaffected. A proper fix
                // requires patching sunset to defer the server EOF until our
                // program actually closes its output.
                if chan.is_some() && !shell_started {
                    let cmd = e.command().ok().map(|s| s.to_string()).unwrap_or_default();
                    if let Some(idx) = alloc_and_spawn_shell() {
                        pty_idx = Some(idx);
                        shell_started = true;
                        // Seed the command + exit into the PTY input queue.
                        for b in cmd.bytes() { crate::pty::master_input_push(idx, b); }
                        crate::pty::master_input_push(idx, b'\n');
                        for b in b"exit\n" { crate::pty::master_input_push(idx, *b); }
                        let _ = e.succeed();
                        crate::binfo!("ssh", "exec '{}' on pty {}", cmd, idx);
                    } else {
                        let _ = e.fail();
                        crate::bwarn!("ssh", "no free pty for exec");
                    }
                } else {
                    let _ = e.fail();
                }
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
                    rx_total += n as u64;
                    progressed = true;
                }
                _ => {}
            }
            // PTY master output (what shell.wasm printed) -> SSH channel.
            // First retry any bytes the channel refused last iteration, then
            // pull fresh bytes from the PTY. Anything still unaccepted goes
            // back to `pending_out` for the next pass — never dropped.
            if !pending_out.is_empty() {
                match runner.write_channel(c, ChanData::Normal, &pending_out) {
                    Ok(w) if w > 0 => {
                        tx_total += w as u64;
                        pending_out.drain(0..w);
                        progressed = true;
                    }
                    _ => {} // channel still not writable; carry pending forward
                }
            }
            if pending_out.is_empty() {
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
                    match runner.write_channel(c, ChanData::Normal, &tx_buf[..filled]) {
                        Ok(w) => {
                            tx_total += w as u64;
                            if w < filled {
                                pending_out.extend_from_slice(&tx_buf[w..filled]);
                            }
                            progressed = true;
                        }
                        Err(_) => {
                            // Channel not writable this pass; stash for retry.
                            pending_out.extend_from_slice(&tx_buf[..filled]);
                        }
                    }
                }
            }
        }

        // 5) Shell finished → close the channel cleanly. Once the spawned
        //    process released its PTY (is_claimed == false) and step 4 has
        //    drained its remaining output, send EOF+CLOSE so the client —
        //    including non-interactive `ssh host cmd` — receives the full
        //    output. (sunset no longer auto-mirrors the client's early EOF.)
        if shell_started && !closing {
            if let Some(idx) = pty_idx {
                if !crate::pty::is_claimed(idx)
                    && crate::pty::master_output_len(idx) == 0
                    && pending_out.is_empty()
                {
                    if let Some(c) = chan.as_ref() {
                        if runner.send_channel_close(c).is_ok() {
                            crate::binfo!("ssh", "shell on pty {} done, closing channel", idx);
                            closing = true;
                            progressed = true;
                        }
                    }
                }
            }
        }

        // Closing and everything queued has reached the socket → done.
        if closing && runner.output_buf().is_empty() {
            break;
        }

        if !progressed {
            Delay::ticks(1).await;
        }
    }
    // Best-effort: flush any still-encoded output (e.g. the EOF/CLOSE) to the
    // socket before tearing the connection down.
    for _ in 0..256 {
        let out = runner.output_buf().len();
        if out == 0 { break; }
        let mut chunk = [0u8; SOCK_CHUNK];
        let n = out.min(SOCK_CHUNK);
        chunk[..n].copy_from_slice(&runner.output_buf()[..n]);
        match sockets::try_send(handle, &chunk[..n]) {
            Some(0) | None => break,
            Some(s)        => runner.consume_output(s),
        }
    }
    // Tidy up: discard channel + close socket.
    if let Some(c) = chan {
        let _ = runner.channel_done(c);
    }
    sockets::close(handle);
    crate::binfo!(
        "ssh", "session done (rx={} tx={} sent={} txdrop={})",
        rx_total, tx_total, sent_total, tx_dropped
    );
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
