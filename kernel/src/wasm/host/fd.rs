//! WASIX file descriptor host fns. Task 2 stubs everything except
//! fd_write (routed to console for FD 1/2).

use wasmi::{Caller, Error, Linker};
use crate::wasm::state::{FdEntry, RuntimeState};
use crate::wasm::host::lifecycle::{wasm_memory, read_u32, write_u32};

pub fn fd_write(
    mut caller: Caller<'_, RuntimeState>,
    fd: i32,
    iovs_ptr: i32,
    iovs_len: i32,
    nwritten_ptr: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    let mut total: u32 = 0;
    for i in 0..iovs_len {
        let iov_at = (iovs_ptr + i * 8) as usize;
        let buf_ptr = read_u32(&mem, &caller, iov_at)? as usize;
        let buf_len = read_u32(&mem, &caller, iov_at + 4)? as usize;
        if buf_len == 0 {
            continue;
        }
        const MAX: usize = 4096;
        let n = buf_len.min(MAX);
        let mut buf = [0u8; MAX];
        mem.read(&caller, buf_ptr, &mut buf[..n])
            .map_err(|e| Error::new(alloc::format!("fd_write mem read: {}", e)))?;

        let fd_entry = caller
            .data()
            .fds
            .get(fd as usize)
            .and_then(|x| x.as_ref())
            .map(|e| match e {
                FdEntry::StdoutConsole => 0u8, // 0 = console
                FdEntry::Vfs(_) => 1u8,         // 1 = vfs (stub)
            });

        match fd_entry {
            Some(0) => {
                if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                    use core::fmt::Write as _;
                    let mut c = crate::console::CONSOLE.lock();
                    let _ = c.write_str(s);
                }
                // non-utf8: silently skip (WASI stdout may emit partial UTF-8)
            }
            Some(1) => {
                // VFS-backed fd: stub EBADF for Task 2; Task 3 wires VFS
                return Ok(8);
            }
            _ => return Ok(8), // EBADF
        }
        total += n as u32;
    }
    write_u32(&mem, &mut caller, nwritten_ptr as usize, total)?;
    Ok(0)
}

pub fn fd_read_stub(
    _: Caller<'_, RuntimeState>,
    _: i32,
    _: i32,
    _: i32,
    _: i32,
) -> Result<i32, Error> {
    Ok(8) // EBADF
}

pub fn fd_close_stub(
    _: Caller<'_, RuntimeState>,
    _: i32,
) -> Result<i32, Error> {
    Ok(0)
}

pub fn fd_seek_stub(
    _: Caller<'_, RuntimeState>,
    _: i32,
    _: i64,
    _: i32,
    _: i32,
) -> Result<i32, Error> {
    Ok(8) // ESPIPE
}

pub fn fd_fdstat_get(
    mut caller: Caller<'_, RuntimeState>,
    _fd: i32,
    stat_ptr: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    // 24-byte zeroed fdstat is fine for stdout/stderr
    let zeros = [0u8; 24];
    mem.write(&mut caller, stat_ptr as usize, &zeros)
        .map_err(|e| Error::new(alloc::format!("fd_fdstat_get mem write: {}", e)))?;
    Ok(0)
}

pub fn fd_prestat_get(
    _: Caller<'_, RuntimeState>,
    _: i32,
    _: i32,
) -> Result<i32, Error> {
    Ok(8) // EBADF — tells wasi-libc no preopened dirs exist
}

pub fn fd_prestat_dir_name(
    _: Caller<'_, RuntimeState>,
    _: i32,
    _: i32,
    _: i32,
) -> Result<i32, Error> {
    Ok(8) // EBADF
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("wasi_snapshot_preview1", "fd_write", fd_write)?
        .func_wrap("wasi_snapshot_preview1", "fd_read", fd_read_stub)?
        .func_wrap("wasi_snapshot_preview1", "fd_close", fd_close_stub)?
        .func_wrap("wasi_snapshot_preview1", "fd_seek", fd_seek_stub)?
        .func_wrap("wasi_snapshot_preview1", "fd_fdstat_get", fd_fdstat_get)?
        .func_wrap("wasi_snapshot_preview1", "fd_prestat_get", fd_prestat_get)?
        .func_wrap("wasi_snapshot_preview1", "fd_prestat_dir_name", fd_prestat_dir_name)?;
    Ok(())
}
