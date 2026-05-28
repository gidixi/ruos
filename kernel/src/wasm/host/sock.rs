//! WASIX / WASI Preview 1 sock_* host fns.
//!
//! sock_accept and sock_connect trap with SuspendReason::Sock* so that
//! Fiber::run can await the async smoltcp futures cooperatively.
//!
//! sock_open / sock_bind / sock_listen are instant (synchronous) and
//! stay as direct calls — they never need to wait.
//!
//! signature: sock_accept(fd: i32, flags: i32, result_fd_ptr: i32) → i32

use wasmi::{Caller, Error, Linker};
use crate::wasm::state::{FdEntry, RuntimeState};

/// sock_accept — trap with SuspendReason::SockAccept so Fiber::run
/// can await the async accept future cooperatively.
pub fn sock_accept(
    caller: Caller<'_, RuntimeState>,
    fd: i32,
    _flags: i32,
    new_fd_ptr: i32,
) -> Result<i32, Error> {
    let idx = match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
        Some(FdEntry::Socket(i)) => *i,
        _ => return Ok(8), // EBADF
    };
    let handle = crate::net::sockets::POOL.handle(idx)
        .ok_or_else(|| Error::i32_exit(-1))?;
    Err(Error::host(crate::wasm::suspend::SuspendReason::SockAccept {
        handle,
        new_fd_ptr: new_fd_ptr as u32,
    }))
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker.func_wrap("wasi_snapshot_preview1", "sock_accept", sock_accept)?;
    Ok(())
}
