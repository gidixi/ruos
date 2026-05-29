//! WASIX path_* host fns.
//! Task 3: path_open traps with SuspendReason::PathOpen; FD allocation
//! done in Fiber::dispatch after the async VFS open completes.

use wasmi::{Caller, Linker, Error};
use crate::wasm::state::RuntimeState;
use crate::wasm::host::lifecycle::wasm_memory;
use crate::vfs::OpenFlags;

// WASI Preview 1 oflags bits
const OFLAGS_CREAT:     i32 = 1 << 0;
const OFLAGS_DIRECTORY: i32 = 1 << 1;
#[allow(dead_code)]
const OFLAGS_EXCL:      i32 = 1 << 2;
const OFLAGS_TRUNC:     i32 = 1 << 3;

// WASI rights bits (fs_rights_base)
const RIGHTS_FD_READ:  i64 = 1 << 1;
const RIGHTS_FD_WRITE: i64 = 1 << 6;

/// path_open(dir_fd, dir_flags, path_ptr, path_len, oflags,
///           fs_rights_base, fs_rights_inheriting, fd_flags,
///           opened_fd_ptr) -> errno
///
/// Maps WASI oflags + fs_rights_base to our internal OpenFlags. Previously
/// hardcoded `CREATE | WRITE | READ` which meant `open("foo", O_RDONLY)`
/// would inadvertently create the file. Closes Step 10.5 F5 + Step 11 F5.
pub fn path_open(
    caller: Caller<'_, RuntimeState>,
    _dir_fd: i32,
    _dir_flags: i32,
    path_ptr: i32,
    path_len: i32,
    oflags: i32,
    fs_rights_base: i64,
    _fs_rights_inheriting: i64,
    _fd_flags: i32,
    opened_fd_ptr: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    let mut path_buf = alloc::vec![0u8; path_len as usize];
    mem.read(&caller, path_ptr as usize, &mut path_buf)
        .map_err(|_| Error::i32_exit(-1))?;
    let path_str = core::str::from_utf8(&path_buf)
        .map_err(|_| Error::i32_exit(-1))?;
    let path: alloc::string::String = if path_str.starts_with('/') {
        alloc::string::String::from(path_str)
    } else {
        let mut p = alloc::string::String::from("/");
        p.push_str(path_str);
        p
    };

    // O_DIRECTORY: not supported yet — readdir() exists but VFS::open
    // returns IsDirectory error for dirs. Return ENOTDIR if the caller
    // wants a directory but our open doesn't model it.
    if oflags & OFLAGS_DIRECTORY != 0 {
        // Let the VFS layer return IsDirectory (mapped to ENOTDIR later);
        // pass READ-only to avoid creating.
    }

    let mut flags = OpenFlags::empty();
    if oflags & OFLAGS_CREAT != 0 { flags |= OpenFlags::CREATE; }
    if oflags & OFLAGS_TRUNC != 0 { flags |= OpenFlags::TRUNCATE; }
    if fs_rights_base & RIGHTS_FD_READ  != 0 { flags |= OpenFlags::READ; }
    if fs_rights_base & RIGHTS_FD_WRITE != 0 { flags |= OpenFlags::WRITE; }
    // POSIX-like default: if neither read nor write bit, assume read.
    if !flags.contains(OpenFlags::READ) && !flags.contains(OpenFlags::WRITE) {
        flags |= OpenFlags::READ;
    }

    Err(Error::host(crate::wasm::suspend::SuspendReason::PathOpen {
        path,
        flags,
        opened_fd_ptr: opened_fd_ptr as u32,
    }))
}

pub fn path_unlink_file(
    _: Caller<'_, RuntimeState>,
    _: i32,
    _: i32,
    _: i32,
) -> Result<i32, Error> {
    Ok(58) // ENOSYS
}

pub fn path_create_directory(
    _: Caller<'_, RuntimeState>,
    _: i32,
    _: i32,
    _: i32,
) -> Result<i32, Error> {
    Ok(58) // ENOSYS
}

pub fn path_remove_directory(
    _: Caller<'_, RuntimeState>,
    _: i32,
    _: i32,
    _: i32,
) -> Result<i32, Error> {
    Ok(58) // ENOSYS
}

pub fn path_filestat_get(
    _: Caller<'_, RuntimeState>,
    _: i32,
    _: i32,
    _: i32,
    _: i32,
    _: i32,
) -> Result<i32, Error> {
    Ok(58) // ENOSYS
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("wasi_snapshot_preview1", "path_open", path_open)?
        .func_wrap("wasi_snapshot_preview1", "path_unlink_file", path_unlink_file)?
        .func_wrap("wasi_snapshot_preview1", "path_create_directory", path_create_directory)?
        .func_wrap("wasi_snapshot_preview1", "path_remove_directory", path_remove_directory)?
        .func_wrap("wasi_snapshot_preview1", "path_filestat_get", path_filestat_get)?;
    Ok(())
}
