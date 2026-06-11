//! `net` host module for Wasmtime-AOT GUI windows — **non-blocking by design**.
//!
//! The window path is fully synchronous (`frame.call` in the compositor loop;
//! no fiber, no epoch): a host fn that blocked on the network would freeze the
//! whole desktop. So every fn here returns immediately and the app polls from
//! its frame loop:
//!
//!   resolve_start(name) → req      resolve_poll(req, ip_out) → 0/1/-1
//!   dial(ip, port) → sock          state(sock) → 0/1/2
//!   read/write(sock, ptr, len) → n | 0 closed | -1 would-block
//!   close(sock)
//!
//! Sockets ride the same kernel pool (`net::sockets::POOL`, smoltcp, driven by
//! `net_poll_task` on the BSP) as the wasmi `ruos.tcp_dial` path. DNS rides
//! `net::dns::resolve` spawned as an embassy task onto the BSP executor
//! (`spawn_on(0, …)`) with the result parked in a slot table this module polls.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};
use wasmtime::{Caller, Linker};

use crate::sync::IrqMutex;

/// Rotating ephemeral local-port source for `net.dial`. The old idx-based port
/// (`49152 + idx`) reused the SAME local port whenever a pool slot was reclaimed,
/// so a fresh connect to a remote we'd just closed collided with the prior
/// 4-tuple still lingering in smoltcp (TIME_WAIT / closing) → intermittent RST /
/// premature close / truncated transfer. A monotonic counter walks all 16384
/// ephemeral ports before any reuse, far outliving any lingering socket.
static NEXT_EPHEMERAL: AtomicU32 = AtomicU32::new(0);

// ---- DNS request slots -----------------------------------------------------

enum Slot {
    Pending,
    Done(Ipv4Address),
    Failed,
}

/// In-flight + completed resolves. Index = the `req` handle handed to the
/// guest; freed (set `None`) when the guest polls a terminal state.
static RESOLVES: IrqMutex<Vec<Option<Slot>>> = IrqMutex::new(Vec::new());

/// Resolve `name` on the BSP executor and park the first A record in the slot.
#[embassy_executor::task(pool_size = 8)]
async fn dns_task(slot: usize, name: String) {
    let result = crate::net::dns::resolve(&name).await;
    let mut g = RESOLVES.lock();
    if let Some(s) = g.get_mut(slot) {
        *s = Some(match result {
            Ok(addrs) if !addrs.is_empty() => Slot::Done(addrs[0]),
            _ => Slot::Failed,
        });
    }
}

// ---- linker ------------------------------------------------------------------

/// Register the `net` module. Generic over the store data (no window state
/// needed — only guest memory and the global socket pool).
pub fn add_to_linker<T: 'static>(linker: &mut Linker<T>) -> wasmtime::Result<()> {
    // net.resolve_start(name_ptr, name_len) -> req (>=0) | -1 (busy/invalid).
    // Kicks an async DNS resolve; poll with resolve_poll.
    linker.func_wrap("net", "resolve_start",
        |mut caller: Caller<'_, T>, name_ptr: i32, name_len: i32| -> i32 {
            if name_len <= 0 || name_len > 253 { return -1; }
            let Some(bytes) = crate::wasm::wt::mem::read(
                &mut caller, name_ptr as u32, name_len as u32) else { return -1; };
            let Ok(name) = core::str::from_utf8(&bytes) else { return -1; };
            let name = String::from(name);
            // Claim a slot first so the task has somewhere to write.
            let slot = {
                let mut g = RESOLVES.lock();
                let i = g.iter().position(|s| s.is_none()).unwrap_or_else(|| {
                    g.push(None);
                    g.len() - 1
                });
                g[i] = Some(Slot::Pending);
                i
            };
            // Spawn onto the BSP executor (core 0 owns net_poll_task + DNS).
            if crate::executor::spawn_on(0, dns_task(slot, name)).is_err() {
                RESOLVES.lock()[slot] = None; // task pool full — free the slot
                return -1;
            }
            slot as i32
        })?;

    // net.resolve_poll(req, ip_out_ptr) -> 1 done (4 IPv4 bytes written) |
    // 0 pending | -1 failed/invalid. Terminal results free the slot.
    linker.func_wrap("net", "resolve_poll",
        |mut caller: Caller<'_, T>, req: i32, ip_out_ptr: i32| -> i32 {
            if req < 0 { return -1; }
            let state = {
                let mut g = RESOLVES.lock();
                match g.get_mut(req as usize) {
                    Some(s @ Some(_)) => match s.as_ref().unwrap() {
                        Slot::Pending => return 0,
                        Slot::Done(ip) => {
                            let ip = *ip;
                            *s = None;
                            Some(ip)
                        }
                        Slot::Failed => {
                            *s = None;
                            None
                        }
                    },
                    _ => return -1,
                }
            };
            match state {
                Some(ip) => {
                    if !crate::wasm::wt::mem::write(&mut caller, ip_out_ptr as u32, &ip.0) {
                        return -1;
                    }
                    1
                }
                None => -1,
            }
        })?;

    // net.dial(ip0..3, port) -> sock (>=0) | -errno. Non-blocking: allocates an
    // Ethernet-side TCP socket and FIRES the connect; poll net.state until 1.
    linker.func_wrap("net", "dial",
        |_caller: Caller<'_, T>, ip0: i32, ip1: i32, ip2: i32, ip3: i32, port: i32| -> i64 {
            if port <= 0 || port > 0xFFFF { return -22; } // EINVAL
            let idx = crate::net::sockets::POOL.alloc_tcp_eth();
            let Some(handle) = crate::net::sockets::POOL.handle(idx) else { return -8; };
            let remote = IpEndpoint::new(
                IpAddress::v4(ip0 as u8, ip1 as u8, ip2 as u8, ip3 as u8),
                port as u16,
            );
            // Rotating ephemeral local port: walk 49152..=65535 monotonically so a
            // reclaimed pool slot never reuses a port still lingering in smoltcp.
            let n = NEXT_EPHEMERAL.fetch_add(1, Ordering::Relaxed);
            let local_port: u16 = 49152u16.wrapping_add((n % 16384) as u16);
            match crate::net::sockets::connect_start(handle, remote, local_port) {
                Ok(()) => idx as i64,
                Err(_) => {
                    crate::net::sockets::close(handle);
                    -111 // ECONNREFUSED-ish: connect could not even start
                }
            }
        })?;

    // net.state(sock) -> 0 connecting | 1 established | 2 closed | -1 invalid.
    linker.func_wrap("net", "state",
        |_caller: Caller<'_, T>, sock: i32| -> i32 {
            if sock < 0 { return -1; }
            match crate::net::sockets::POOL.handle(sock as usize) {
                Some(h) => crate::net::sockets::state_of(h) as i32,
                None => -1,
            }
        })?;

    // net.read(sock, ptr, len) -> n>0 bytes | 0 peer closed | -1 would-block |
    // -2 invalid. Copies at most RX_BUF_SIZE bytes per call (socket RX buffer).
    linker.func_wrap("net", "read",
        |mut caller: Caller<'_, T>, sock: i32, ptr: i32, len: i32| -> i32 {
            if sock < 0 || len <= 0 { return -2; }
            let Some(h) = crate::net::sockets::POOL.handle(sock as usize) else { return -2; };
            let want = (len as usize).min(crate::net::sockets::RX_BUF_SIZE);
            let mut buf = alloc::vec![0u8; want];
            match crate::net::sockets::try_recv(h, &mut buf) {
                Some(0) => 0,
                Some(n) => {
                    if !crate::wasm::wt::mem::write(&mut caller, ptr as u32, &buf[..n]) {
                        return -2;
                    }
                    n as i32
                }
                None => -1,
            }
        })?;

    // net.write(sock, ptr, len) -> n>0 bytes accepted | 0 closed for write |
    // -1 would-block (TX full) | -2 invalid.
    linker.func_wrap("net", "write",
        |mut caller: Caller<'_, T>, sock: i32, ptr: i32, len: i32| -> i32 {
            if sock < 0 || len <= 0 { return -2; }
            let Some(h) = crate::net::sockets::POOL.handle(sock as usize) else { return -2; };
            let Some(buf) = crate::wasm::wt::mem::read(&mut caller, ptr as u32, len as u32)
                else { return -2; };
            match crate::net::sockets::try_send(h, &buf) {
                Some(n) => n as i32,
                None => -1,
            }
        })?;

    // net.close(sock): graceful close + release the pool slot. The socket is
    // reclaimed (removed from the SocketSet, slot reusable) once the FIN
    // handshake reaches Closed — required for real pages, which open dozens
    // of connections per load. The handle is invalid after this call.
    linker.func_wrap("net", "close",
        |_caller: Caller<'_, T>, sock: i32| {
            if sock < 0 { return; }
            crate::net::sockets::POOL.release(sock as usize);
        })?;

    Ok(())
}
