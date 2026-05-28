//! WASIX path_* host fns.
//! Task 3: path_open traps with SuspendReason::PathOpen; FD allocation
//! done in Fiber::dispatch after the async VFS open completes.

use wasmi::{Caller, Linker, Error};
use crate::wasm::state::RuntimeState;
use crate::wasm::host::lifecycle::wasm_memory;
use crate::vfs::OpenFlags;

/// path_open(dir_fd, dir_flags, path_ptr, path_len, oflags,
///           fs_rights_base, fs_rights_inheriting, fd_flags,
///           opened_fd_ptr) -> errno
pub fn path_open(
    caller: Caller<'_, RuntimeState>,
    _dir_fd: i32,
    _dir_flags: i32,
    path_ptr: i32,
    path_len: i32,
    _oflags: i32,
    _fs_rights_base: i64,
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
    let flags = OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::READ;
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
