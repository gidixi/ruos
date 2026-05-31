//! WASIX-style termios host fns under module "ruos".

use wasmi::{Caller, Linker, Error};
use crate::wasm::state::{RuntimeState, FdEntry};

pub fn tcgetattr(
    mut caller: Caller<'_, RuntimeState>,
    fd: i32,
    termios_ptr: i32,
) -> Result<i32, Error> {
    let pty_idx = match fd_to_pty(&caller, fd) {
        Some(idx) => idx,
        None => return Ok(25), // ENOTTY
    };
    let pair = crate::pty::pair(pty_idx);
    let g = pair.lock();
    let t = g.termios;
    drop(g);
    let bytes = unsafe {
        core::slice::from_raw_parts(
            &t as *const _ as *const u8,
            core::mem::size_of::<crate::pty::termios::Termios>(),
        )
    };
    if let Err(e) = crate::wasm::host::mem::guest_write(&mut caller, termios_ptr, bytes) {
        return Ok(e);
    }
    Ok(0)
}

pub fn tcsetattr(
    caller: Caller<'_, RuntimeState>,
    fd: i32,
    _action: i32,
    termios_ptr: i32,
) -> Result<i32, Error> {
    let pty_idx = match fd_to_pty(&caller, fd) {
        Some(idx) => idx,
        None => return Ok(25), // ENOTTY
    };
    let mut termios = crate::pty::termios::Termios::default_cooked();
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(
            &mut termios as *mut _ as *mut u8,
            core::mem::size_of::<crate::pty::termios::Termios>(),
        )
    };
    if let Err(e) = crate::wasm::host::mem::guest_read_into(&caller, termios_ptr, bytes) {
        return Ok(e);
    }
    crate::pty::pair(pty_idx).lock().termios = termios;
    Ok(0)
}

pub fn isatty(
    caller: Caller<'_, RuntimeState>,
    fd: i32,
) -> Result<i32, Error> {
    Ok(if fd_to_pty(&caller, fd).is_some() { 1 } else { 0 })
}

/// Map wasm-side FD → PTY idx if and only if it backs a PtySlaveFile.
fn fd_to_pty(caller: &Caller<'_, RuntimeState>, fd: i32) -> Option<usize> {
    let entry = caller.data().fds.get(fd as usize)?.as_ref()?;
    let vfs_fd = match entry {
        FdEntry::Vfs(f) => *f,
        _ => return None,
    };
    // Peek into the global FDS table to find the FileImpl variant.
    let t = crate::vfs::fd::FDS.lock();
    let slot = t.get(vfs_fd as usize)?.as_ref()?;
    match &slot.file {
        crate::vfs::file::FileImpl::PtySlave(p) => Some(p.idx),
        _ => None,
    }
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("ruos", "tcgetattr", tcgetattr)?
        .func_wrap("ruos", "tcsetattr", tcsetattr)?
        .func_wrap("ruos", "isatty", isatty)?;
    Ok(())
}
