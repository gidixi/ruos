//! WASIX path_* host fns. Resolve a wasm-side path to a VFS Fd,
//! allocate a wasm-side FD slot, return it to the wasm.

use wasmi::{Caller, Linker, Error};
use alloc::string::String;
use crate::wasm::state::{FdEntry, RuntimeState};
use crate::wasm::host::lifecycle::{wasm_memory, write_u32};
use crate::vfs::{self, OpenFlags};

/// path_open(dir_fd, dir_flags, path_ptr, path_len, oflags,
///           fs_rights_base, fs_rights_inheriting, fd_flags,
///           opened_fd_ptr) -> errno
pub fn path_open(
    mut caller: Caller<'_, RuntimeState>,
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
    // Read the path from wasm memory before any mutable borrow of state.
    let mem = wasm_memory(&caller)?;
    let mut path_buf = alloc::vec![0u8; path_len as usize];
    mem.read(&caller, path_ptr as usize, &mut path_buf)
        .map_err(|_| Error::new("path_open: mem read failed"))?;
    let path_str = core::str::from_utf8(&path_buf)
        .map_err(|_| Error::new("path_open: invalid utf8"))?;
    let path: String = if path_str.starts_with('/') {
        String::from(path_str)
    } else {
        let mut p = String::from("/");
        p.push_str(path_str);
        p
    };

    // Open via VFS (async bridged with embassy_futures::block_on).
    let res: Result<vfs::Fd, vfs::VfsError> = embassy_futures::block_on(
        vfs::open(&path, OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::READ),
    );
    let vfd = match res {
        Ok(f) => f,
        Err(_) => return Ok(44), // ENOENT
    };

    // Allocate a slot in the wasm-side FD table (skip 0/1/2).
    // Use a block so the mutable borrow of state is released before
    // write_u32 borrows caller again.
    let wfd: usize = {
        let state = caller.data_mut();
        let mut found: Option<usize> = None;
        for (i, slot) in state.fds.iter_mut().enumerate().skip(3) {
            if slot.is_none() {
                *slot = Some(FdEntry::Vfs(vfd));
                found = Some(i);
                break;
            }
        }
        match found {
            Some(w) => w,
            None => {
                state.fds.push(Some(FdEntry::Vfs(vfd)));
                state.fds.len() - 1
            }
        }
    };

    // Write the wasm-side FD back to wasm memory.
    write_u32(&mem, &mut caller, opened_fd_ptr as usize, wfd as u32)?;
    Ok(0)
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
