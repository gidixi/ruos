//! WASIX / WASI Preview 1 sock_* host fns.
//!
//! Our implementation uses the "socket activation" model: the kernel
//! pre-opens sockets and hands them to the wasm module as FdEntry::Socket
//! at well-known FD numbers (e.g. FD 4). The wasm code never calls
//! sock_open / sock_bind / sock_listen / sock_connect — the kernel did all
//! that before instantiating the module.
//!
//! The only sock_* syscall the server.wasm module actually needs is
//! sock_accept (type[3] = (i32, i32, i32) → i32).
//!
//! signature: sock_accept(fd: i32, flags: i32, result_fd_ptr: i32) → i32
//! returns: 0 on success, errno otherwise.
//! on success writes the accepted FD (u32 LE) to result_fd_ptr.

use wasmi::{Caller, Error, Linker};
use crate::wasm::state::{FdEntry, RuntimeState};
use crate::wasm::host::lifecycle::{wasm_memory, write_u32};

/// sock_accept — wait until the pre-opened listening socket at `fd`
/// transitions to Established, then write `fd` back as the accepted FD.
///
/// In our single-connection demo the listen socket IS the accepted socket
/// (smoltcp transitions it Listen→Established on the first SYN). We
/// return the same FD number to the caller as the "new" connected FD.
pub fn sock_accept(
    mut caller: Caller<'_, RuntimeState>,
    fd: i32,
    _flags: i32,
    result_fd_ptr: i32,
) -> Result<i32, Error> {
    // Resolve fd → socket pool index.
    let idx = match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
        Some(FdEntry::Socket(i)) => *i,
        _ => return Ok(8), // EBADF
    };
    let handle = crate::net::sockets::POOL.handle(idx)
        .ok_or_else(|| Error::i32_exit(-1))?;

    // Synchronously poll until the socket reaches Established.
    crate::net::sockets::accept_sync(handle)
        .map_err(|e| Error::new(alloc::format!("sock_accept: {}", e)))?;

    // Write the accepted FD (same as listen_fd — socket is now connected).
    let mem = wasm_memory(&caller)?;
    write_u32(&mem, &mut caller, result_fd_ptr as usize, fd as u32)?;
    Ok(0)
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker.func_wrap("wasi_snapshot_preview1", "sock_accept", sock_accept)?;
    Ok(())
}
