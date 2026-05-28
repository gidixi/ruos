//! WASIX file descriptor host fns.
//! Task 2: fd_write (console), stubs for read/seek/close.
//! Task 3: real fd_read / fd_seek / fd_close + VFS dispatch in fd_write.

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
                FdEntry::Stdin => 3u8,          // 3 = stdin (not writable)
                FdEntry::StdoutConsole => 0u8,  // 0 = console
                FdEntry::Vfs(_) => 1u8,         // 1 = vfs
                FdEntry::Socket(_) => 2u8,      // 2 = socket
            });

        match fd_entry {
            Some(0) => {
                if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                    use core::fmt::Write as _;
                    let mut c = crate::console::CONSOLE.lock();
                    let _ = c.write_str(s);
                }
                // non-utf8: silently skip
                total += n as u32;
            }
            Some(1) => {
                // VFS-backed fd: dispatch to VFS write.
                let vfd = match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
                    Some(FdEntry::Vfs(v)) => *v,
                    _ => return Ok(8),
                };
                let written = embassy_futures::block_on(crate::vfs::write(vfd, &buf[..n]))
                    .map_err(|_| Error::i32_exit(-1))?;
                total += written as u32;
            }
            Some(2) => {
                // Socket FD: dispatch to net::sockets send.
                let idx = match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
                    Some(FdEntry::Socket(i)) => *i,
                    _ => return Ok(8),
                };
                let handle = crate::net::sockets::POOL.handle(idx)
                    .ok_or_else(|| Error::i32_exit(-1))?;
                let written = crate::net::sockets::send_sync(handle, &buf[..n])
                    .map_err(|e| Error::new(alloc::format!("fd_write socket: {}", e)))?;
                total += written as u32;
            }
            _ => return Ok(8), // EBADF
        }
    }
    write_u32(&mem, &mut caller, nwritten_ptr as usize, total)?;
    Ok(0)
}

pub fn fd_read(
    mut caller: Caller<'_, RuntimeState>,
    fd: i32,
    iovs_ptr: i32,
    iovs_len: i32,
    nread_ptr: i32,
) -> Result<i32, Error> {
    // Classify the fd up-front (avoid borrowing caller across async ops).
    let fd_kind = match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
        Some(FdEntry::Stdin) => 0u8,
        Some(FdEntry::Vfs(v)) => { let _ = *v; 1u8 }
        Some(FdEntry::Socket(i)) => { let _ = *i; 2u8 }
        _ => return Ok(8), // EBADF
    };

    // Stdin: read exactly 1 byte from the keyboard queue, fill the first
    // non-empty iov and return immediately.
    if fd_kind == 0 {
        let mem = wasm_memory(&caller)?;
        for i in 0..iovs_len {
            let iov_at = (iovs_ptr + i * 8) as usize;
            let buf_ptr = read_u32(&mem, &caller, iov_at)? as usize;
            let buf_len = read_u32(&mem, &caller, iov_at + 4)? as usize;
            if buf_len == 0 {
                continue;
            }
            let b = embassy_futures::block_on(crate::keyboard::queue::read_char());
            mem.write(&mut caller, buf_ptr, &[b])
                .map_err(|_| Error::i32_exit(-1))?;
            let mem2 = wasm_memory(&caller)?;
            write_u32(&mem2, &mut caller, nread_ptr as usize, 1)?;
            return Ok(0);
        }
        // All iovs were zero-length.
        let mem2 = wasm_memory(&caller)?;
        write_u32(&mem2, &mut caller, nread_ptr as usize, 0)?;
        return Ok(0);
    }

    // Socket read.
    if fd_kind == 2 {
        let idx = match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
            Some(FdEntry::Socket(i)) => *i,
            _ => return Ok(8),
        };
        let handle = crate::net::sockets::POOL.handle(idx)
            .ok_or_else(|| Error::i32_exit(-1))?;
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
            let mut kbuf = alloc::vec![0u8; n];
            let read_n = crate::net::sockets::recv_sync(handle, &mut kbuf)
                .map_err(|e| Error::new(alloc::format!("fd_read socket: {}", e)))?;
            mem.write(&mut caller, buf_ptr, &kbuf[..read_n])
                .map_err(|_| Error::i32_exit(-1))?;
            total += read_n as u32;
            if read_n < n {
                break;
            }
        }
        let mem2 = wasm_memory(&caller)?;
        write_u32(&mem2, &mut caller, nread_ptr as usize, total)?;
        return Ok(0);
    }

    // VFS-backed read.
    let vfd = match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
        Some(FdEntry::Vfs(v)) => *v,
        _ => return Ok(8),
    };
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
        let mut tmp = [0u8; MAX];
        let read_n = embassy_futures::block_on(crate::vfs::read(vfd, &mut tmp[..n]))
            .map_err(|_| Error::i32_exit(-1))?;
        mem.write(&mut caller, buf_ptr, &tmp[..read_n])
            .map_err(|_| Error::i32_exit(-1))?;
        total += read_n as u32;
        if read_n < n {
            break;
        }
    }
    let mem2 = wasm_memory(&caller)?;
    write_u32(&mem2, &mut caller, nread_ptr as usize, total)?;
    Ok(0)
}

pub fn fd_seek(
    mut caller: Caller<'_, RuntimeState>,
    fd: i32,
    offset: i64,
    whence: i32,
    newoffset_ptr: i32,
) -> Result<i32, Error> {
    let vfd = match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
        Some(FdEntry::Vfs(v)) => *v,
        _ => return Ok(8), // EBADF
    };
    let w = match whence {
        0 => crate::vfs::Whence::Set,
        1 => crate::vfs::Whence::Cur,
        2 => crate::vfs::Whence::End,
        _ => return Ok(28), // EINVAL
    };
    let new_off = embassy_futures::block_on(crate::vfs::seek(vfd, offset, w))
        .map_err(|_| Error::i32_exit(-1))?;
    let mem = wasm_memory(&caller)?;
    mem.write(&mut caller, newoffset_ptr as usize, &(new_off as u64).to_le_bytes())
        .map_err(|_| Error::i32_exit(-1))?;
    Ok(0)
}

pub fn fd_close(
    mut caller: Caller<'_, RuntimeState>,
    fd: i32,
) -> Result<i32, Error> {
    let taken = caller
        .data_mut()
        .fds
        .get_mut(fd as usize)
        .and_then(|x| x.take());
    match taken {
        Some(FdEntry::Vfs(vfd)) => {
            let _ = embassy_futures::block_on(crate::vfs::close(vfd));
        }
        Some(other) => {
            // Restore non-VFS entry (don't drop stdin/stdout).
            caller.data_mut().fds[fd as usize] = Some(other);
        }
        None => return Ok(8), // EBADF
    }
    Ok(0)
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

/// Expose a single preopen at fd=3: "/" (root of tmpfs).
/// wasi-libc requires at least one preopen to allow path_open calls for
/// absolute paths. We return type=0 (dir), name_len=1 for fd=3;
/// EBADF for any fd >= 4.
pub fn fd_prestat_get(
    mut caller: Caller<'_, RuntimeState>,
    fd: i32,
    stat_ptr: i32,
) -> Result<i32, Error> {
    if fd != 3 {
        return Ok(8); // EBADF — no more preopens
    }
    let mem = wasm_memory(&caller)?;
    // wasi_prestat_t: u32 type=0 (dir), u32 name_len=1 (for "/")
    let mut stat = [0u8; 8];
    stat[0..4].copy_from_slice(&0u32.to_le_bytes()); // type = PREOPENTYPE_DIR
    stat[4..8].copy_from_slice(&1u32.to_le_bytes()); // name_len = 1
    mem.write(&mut caller, stat_ptr as usize, &stat)
        .map_err(|e| Error::new(alloc::format!("fd_prestat_get write: {}", e)))?;
    Ok(0)
}

/// Return the preopen name for fd=3 → "/".
pub fn fd_prestat_dir_name(
    mut caller: Caller<'_, RuntimeState>,
    fd: i32,
    path_ptr: i32,
    _path_len: i32,
) -> Result<i32, Error> {
    if fd != 3 {
        return Ok(8); // EBADF
    }
    let mem = wasm_memory(&caller)?;
    mem.write(&mut caller, path_ptr as usize, b"/")
        .map_err(|e| Error::new(alloc::format!("fd_prestat_dir_name write: {}", e)))?;
    Ok(0)
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("wasi_snapshot_preview1", "fd_write", fd_write)?
        .func_wrap("wasi_snapshot_preview1", "fd_read", fd_read)?
        .func_wrap("wasi_snapshot_preview1", "fd_close", fd_close)?
        .func_wrap("wasi_snapshot_preview1", "fd_seek", fd_seek)?
        .func_wrap("wasi_snapshot_preview1", "fd_fdstat_get", fd_fdstat_get)?
        .func_wrap("wasi_snapshot_preview1", "fd_prestat_get", fd_prestat_get)?
        .func_wrap("wasi_snapshot_preview1", "fd_prestat_dir_name", fd_prestat_dir_name)?;
    Ok(())
}
