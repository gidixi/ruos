//! WASIX file descriptor host fns.
//! Task 3: all VFS/Stdin arms trap with SuspendReason; embassy_futures removed.

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
    // Socket arm: trap with SuspendReason::SockSend (single iov only).
    if let Some(FdEntry::Socket(idx)) = caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
        if iovs_len != 1 {
            return Ok(28); // EINVAL: multi-iov socket writes not supported
        }
        let idx = *idx;
        let handle = crate::net::sockets::POOL.handle(idx)
            .ok_or_else(|| Error::i32_exit(-1))?;
        let mem = wasm_memory(&caller)?;
        let buf_ptr = read_u32(&mem, &caller, iovs_ptr as usize)?;
        let buf_len = read_u32(&mem, &caller, iovs_ptr as usize + 4)?;
        const MAX: usize = 4096;
        let mut buf = [0u8; MAX];
        let n = (buf_len as usize).min(MAX);
        mem.read(&caller, buf_ptr as usize, &mut buf[..n])
            .map_err(|_| Error::i32_exit(-1))?;
        let bytes_owned = buf[..n].to_vec();
        return Err(Error::host(crate::wasm::suspend::SuspendReason::SockSend {
            handle,
            bytes: bytes_owned,
            nsent_ptr: nwritten_ptr as u32,
        }));
    }

    // VFS arm: trap with SuspendReason::VfsWrite (single iov only).
    if let Some(FdEntry::Vfs(vfd)) = caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
        if iovs_len != 1 {
            return Ok(28); // EINVAL: multi-iov VFS writes not supported
        }
        let vfd = *vfd;
        let mem = wasm_memory(&caller)?;
        let buf_ptr = read_u32(&mem, &caller, iovs_ptr as usize)?;
        let buf_len = read_u32(&mem, &caller, iovs_ptr as usize + 4)?;
        const MAX: usize = 4096;
        let mut buf = [0u8; MAX];
        let n = (buf_len as usize).min(MAX);
        mem.read(&caller, buf_ptr as usize, &mut buf[..n])
            .map_err(|_| Error::i32_exit(-1))?;
        let bytes_owned = buf[..n].to_vec();
        return Err(Error::host(crate::wasm::suspend::SuspendReason::VfsWrite {
            fd: vfd,
            bytes: bytes_owned,
            nwritten_ptr: nwritten_ptr as u32,
        }));
    }

    // Console (stdout/stderr) arm: write all iovs directly.
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

        let is_console = matches!(
            caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()),
            Some(FdEntry::StdoutConsole)
        );

        if is_console {
            if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                use core::fmt::Write as _;
                let mut c = crate::console::CONSOLE.lock();
                let _ = c.write_str(s);
            }
            // non-utf8: silently skip
            total += n as u32;
        } else {
            return Ok(8); // EBADF
        }
    }
    write_u32(&mem, &mut caller, nwritten_ptr as usize, total)?;
    Ok(0)
}

pub fn fd_read(
    caller: Caller<'_, RuntimeState>,
    fd: i32,
    iovs_ptr: i32,
    iovs_len: i32,
    nread_ptr: i32,
) -> Result<i32, Error> {
    // Socket arm: trap with SuspendReason::SockRecv (single iov only).
    if let Some(FdEntry::Socket(idx)) = caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
        if iovs_len != 1 {
            return Ok(28); // EINVAL
        }
        let idx = *idx;
        let handle = crate::net::sockets::POOL.handle(idx)
            .ok_or_else(|| Error::i32_exit(-1))?;
        let mem = wasm_memory(&caller)?;
        let buf_ptr = read_u32(&mem, &caller, iovs_ptr as usize)?;
        let buf_len = read_u32(&mem, &caller, iovs_ptr as usize + 4)?;
        return Err(Error::host(crate::wasm::suspend::SuspendReason::SockRecv {
            handle,
            buf_ptr,
            max_len: buf_len as usize,
            nrecv_ptr: nread_ptr as u32,
        }));
    }

    // VFS arm: trap with SuspendReason::VfsRead (single iov only).
    if let Some(FdEntry::Vfs(vfd)) = caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
        if iovs_len != 1 {
            return Ok(28); // EINVAL
        }
        let vfd = *vfd;
        let mem = wasm_memory(&caller)?;
        let buf_ptr = read_u32(&mem, &caller, iovs_ptr as usize)?;
        let buf_len = read_u32(&mem, &caller, iovs_ptr as usize + 4)?;
        return Err(Error::host(crate::wasm::suspend::SuspendReason::VfsRead {
            fd: vfd,
            buf_ptr,
            max_len: buf_len as usize,
            nread_ptr: nread_ptr as u32,
        }));
    }

    Ok(8) // EBADF
}

pub fn fd_seek(
    caller: Caller<'_, RuntimeState>,
    fd: i32,
    offset: i64,
    whence: i32,
    newoffset_ptr: i32,
) -> Result<i32, Error> {
    let entry = match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
        Some(FdEntry::Vfs(vfd)) => *vfd,
        _ => return Ok(8), // EBADF
    };
    let w = match whence {
        0 => crate::vfs::Whence::Set,
        1 => crate::vfs::Whence::Cur,
        2 => crate::vfs::Whence::End,
        _ => return Ok(28), // EINVAL
    };
    Err(Error::host(crate::wasm::suspend::SuspendReason::VfsSeek {
        fd: entry,
        offset,
        whence: w,
        newoffset_ptr: newoffset_ptr as u32,
    }))
}

pub fn fd_close(
    mut caller: Caller<'_, RuntimeState>,
    fd: i32,
) -> Result<i32, Error> {
    let taken = caller.data_mut().fds.get_mut(fd as usize).and_then(|x| x.take());
    match taken {
        Some(FdEntry::Vfs(vfd)) => {
            Err(Error::host(crate::wasm::suspend::SuspendReason::VfsClose { fd: vfd }))
        }
        Some(other) => {
            // Restore non-VFS entry (don't drop StdoutConsole/Socket).
            caller.data_mut().fds[fd as usize] = Some(other);
            Ok(0)
        }
        None => Ok(8), // EBADF
    }
}

/// fd_filestat_get: return minimal wasi_filestat_t (64 bytes).
/// Looks up the VFS Fd's underlying File and queries `stat()` for kind +
/// size — closes Step 11 F7 (was hardcoded size=0, which forced std::fs
/// callers like cat.wasm into a slow read-loop fallback).
pub fn fd_filestat_get(
    mut caller: Caller<'_, RuntimeState>,
    fd: i32,
    buf_ptr: i32,
) -> Result<i32, Error> {
    use crate::wasm::state::FdEntry;
    let vfs_fd = match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
        Some(FdEntry::Vfs(f)) => *f,
        Some(FdEntry::StdoutConsole) => {
            // Character device, size 0.
            let mem = wasm_memory(&caller)?;
            let mut stat = [0u8; 64];
            stat[16] = 2; // CHARACTER_DEVICE
            mem.write(&mut caller, buf_ptr as usize, &stat)
                .map_err(|e| Error::new(alloc::format!("fd_filestat_get write: {}", e)))?;
            return Ok(0);
        }
        _ => return Ok(8), // EBADF
    };
    // Run stat synchronously: all current File impls' stat futures
    // complete in a single poll (no real I/O suspends).
    let st = match crate::vfs::block_on(crate::vfs::stat_fd(vfs_fd)) {
        Ok(s) => s,
        Err(_) => return Ok(8),
    };
    let filetype: u8 = match st.kind {
        crate::vfs::VfsKind::Reg    => 4, // REGULAR_FILE
        crate::vfs::VfsKind::Dir    => 3, // DIRECTORY
        crate::vfs::VfsKind::Device => 2, // CHARACTER_DEVICE
    };
    let mem = wasm_memory(&caller)?;
    // wasi_filestat_t layout (64 bytes):
    //   dev(8) ino(8) filetype(1)+pad(7) nlink(8) size(8) atim(8) mtim(8) ctim(8)
    let mut stat = [0u8; 64];
    stat[16] = filetype;
    stat[32..40].copy_from_slice(&st.size.to_le_bytes());
    mem.write(&mut caller, buf_ptr as usize, &stat)
        .map_err(|e| Error::new(alloc::format!("fd_filestat_get write: {}", e)))?;
    Ok(0)
}

pub fn fd_fdstat_get(
    mut caller: Caller<'_, RuntimeState>,
    fd: i32,
    stat_ptr: i32,
) -> Result<i32, Error> {
    use crate::wasm::state::FdEntry;
    // Resolve filetype + grant full rights so wasi-libc allows read/write.
    // wasi_fdstat_t layout (24 bytes):
    //   fs_filetype: u8 (0)
    //   pad: u8 (1)
    //   fs_flags: u16 (2)
    //   pad: u32 (4..8)
    //   fs_rights_base: u64 (8..16)
    //   fs_rights_inheriting: u64 (16..24)
    // FD 3 is wasi-libc's preopen root "/" — virtual, not in fds Vec.
    let filetype: u8 = if fd == 3 {
        3 // DIRECTORY
    } else {
        match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
            Some(FdEntry::Vfs(vfs_fd)) => {
                match crate::vfs::block_on(crate::vfs::stat_fd(*vfs_fd)) {
                    Ok(s) => match s.kind {
                        crate::vfs::VfsKind::Reg    => 4, // REGULAR_FILE
                        crate::vfs::VfsKind::Dir    => 3, // DIRECTORY
                        crate::vfs::VfsKind::Device => 2, // CHARACTER_DEVICE
                    },
                    Err(_) => 0,
                }
            }
            Some(FdEntry::StdoutConsole) => 2,
            Some(FdEntry::Socket(_))     => 7, // SOCKET_STREAM
            _ => return Ok(8), // EBADF
        }
    };

    let mut stat = [0u8; 24];
    stat[0] = filetype;
    // Grant all rights — we don't enforce ACL on the kernel side.
    stat[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
    stat[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
    let mem = wasm_memory(&caller)?;
    mem.write(&mut caller, stat_ptr as usize, &stat)
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
        .func_wrap("wasi_snapshot_preview1", "fd_filestat_get", fd_filestat_get)?
        .func_wrap("wasi_snapshot_preview1", "fd_prestat_get", fd_prestat_get)?
        .func_wrap("wasi_snapshot_preview1", "fd_prestat_dir_name", fd_prestat_dir_name)?;
    Ok(())
}
