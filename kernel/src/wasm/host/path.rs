//! WASIX path_* host fns.
//! Task 3: path_open traps with SuspendReason::PathOpen; FD allocation
//! done in Fiber::dispatch after the async VFS open completes.

use wasmi::{Caller, Linker, Error};
use crate::wasm::state::{FdEntry, RuntimeState};
use crate::vfs::OpenFlags;

/// Resolve a (possibly relative) `path` against the base implied by `dir_fd`.
///
/// WASI paths are relative to a directory fd. When `dir_fd` refers to an open
/// directory (`FdEntry::Dir`), relative paths resolve against THAT directory —
/// this is what makes `std::fs::read_dir` work: after `fd_readdir`, std calls
/// `path_filestat_get(dir_fd, "<entry>")` to stat each entry, expecting it
/// resolved against the directory, not the cwd.
///
/// For the virtual preopen fd 3 (and any non-dir fd) we resolve against the
/// preopen ROOT "/", NOT the kernel cwd. Rationale: wasi-libc resolves the
/// program's path against the "/" preopen and passes the kernel the REMAINDER
/// after the preopen prefix — so an absolute `/bin/ls.wasm` arrives here as
/// `bin/ls.wasm` (leading slash stripped). Re-applying the kernel cwd as base
/// double-counted it: at cwd `/bin`, `bin/ls.wasm` → `/bin/bin/ls.wasm` → ENOENT,
/// which broke EVERY external command (and every absolute path) at any cwd ≠ "/".
/// Rooting at "/" makes absolute paths correct at any cwd. (It works at cwd "/"
/// identically to before, since base was "/" there too.)
///
/// CWD-RELATIVE PATHS: a tool that opens a genuinely cwd-relative path (e.g.
/// `cat foo.txt` while the shell is in /mnt) is handled by syncing the GUEST libc
/// cwd, not by this function. The kernel injects `PWD=<cwd>` into each child's
/// environ (`Fiber::set_cwd`); the tool's `ruos_rt::init()` reads it and calls
/// `set_current_dir`, so wasi-libc roots the relative path at the real cwd BEFORE
/// stripping the preopen prefix — and it then arrives here already correct. This
/// keeps `resolve_at` stateless (base "/") while both absolute and cwd-relative
/// paths resolve correctly. (A tool that does NOT link ruos-rt sees cwd "/".)
fn resolve_at(caller: &Caller<'_, RuntimeState>, dir_fd: i32, path: &str) -> alloc::string::String {
    let base = match caller.data().fds.get(dir_fd as usize).and_then(|x| x.as_ref()) {
        Some(FdEntry::Dir(p)) => p.clone(),
        _ => alloc::string::String::from("/"),
    };
    crate::wasm::host::proc::resolve_cwd(&base, path)
}

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
    dir_fd: i32,
    _dir_flags: i32,
    path_ptr: i32,
    path_len: i32,
    oflags: i32,
    fs_rights_base: i64,
    _fs_rights_inheriting: i64,
    _fd_flags: i32,
    opened_fd_ptr: i32,
) -> Result<i32, Error> {
    let path_buf = match crate::wasm::host::mem::guest_read(&caller, path_ptr, path_len) {
        Ok(b) => b,
        Err(e) => return Ok(e),
    };
    let path_str = core::str::from_utf8(&path_buf)
        .map_err(|_| Error::i32_exit(-1))?;
    // Resolve relative path against dir_fd's directory (or cwd for the
    // preopen / non-dir fds). See resolve_at.
    let path = resolve_at(&caller, dir_fd, path_str);

    // Capability check: reject paths outside the task's grant.
    if !caller.data().grants(&path) { return Ok(76); } // ENOTCAPABLE

    // O_DIRECTORY: model the open directory as a first-class fd. Trap with
    // OpenDir, which stats the path and (if it's a directory) allocates an
    // FdEntry::Dir. This is the fd that wasi-libc's fdopendir/fd_readdir
    // operates on.
    if oflags & OFLAGS_DIRECTORY != 0 {
        return Err(Error::host(crate::wasm::suspend::SuspendReason::OpenDir {
            path,
            opened_fd_ptr: opened_fd_ptr as u32,
        }));
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

fn read_path(
    caller: &Caller<'_, RuntimeState>,
    dir_fd: i32,
    path_ptr: i32,
    path_len: i32,
) -> Result<alloc::string::String, Error> {
    let buf = crate::wasm::host::mem::guest_read(caller, path_ptr, path_len)
        .map_err(|_| Error::i32_exit(-1))?;
    let s = core::str::from_utf8(&buf).map_err(|_| Error::i32_exit(-1))?;
    Ok(resolve_at(caller, dir_fd, s))
}

pub fn path_unlink_file(
    caller: Caller<'_, RuntimeState>,
    dir_fd: i32,
    path_ptr: i32,
    path_len: i32,
) -> Result<i32, Error> {
    let path = read_path(&caller, dir_fd, path_ptr, path_len)?;
    if !caller.data().grants(&path) { return Ok(76); } // ENOTCAPABLE
    Err(Error::host(crate::wasm::suspend::SuspendReason::PathUnlink { path }))
}

pub fn path_create_directory(
    caller: Caller<'_, RuntimeState>,
    dir_fd: i32,
    path_ptr: i32,
    path_len: i32,
) -> Result<i32, Error> {
    let path = read_path(&caller, dir_fd, path_ptr, path_len)?;
    if !caller.data().grants(&path) { return Ok(76); } // ENOTCAPABLE
    Err(Error::host(crate::wasm::suspend::SuspendReason::PathMkdir { path }))
}

pub fn path_remove_directory(
    caller: Caller<'_, RuntimeState>,
    dir_fd: i32,
    path_ptr: i32,
    path_len: i32,
) -> Result<i32, Error> {
    let path = read_path(&caller, dir_fd, path_ptr, path_len)?;
    if !caller.data().grants(&path) { return Ok(76); } // ENOTCAPABLE
    Err(Error::host(crate::wasm::suspend::SuspendReason::PathRmdir { path }))
}

/// path_filestat_get(dir_fd, flags, path_ptr, path_len, buf_ptr) -> errno
/// Writes a 64-byte wasi_filestat_t at buf_ptr — same layout as fd_filestat_get.
pub fn path_filestat_get(
    caller: Caller<'_, RuntimeState>,
    dir_fd: i32,
    _flags: i32,
    path_ptr: i32,
    path_len: i32,
    buf_ptr: i32,
) -> Result<i32, Error> {
    let path = read_path(&caller, dir_fd, path_ptr, path_len)?;
    if !caller.data().grants(&path) { return Ok(76); } // ENOTCAPABLE
    Err(Error::host(crate::wasm::suspend::SuspendReason::PathFilestat {
        path,
        buf_ptr: buf_ptr as u32,
    }))
}

/// path_rename(old_fd, old_path_ptr, old_path_len, new_fd, new_path_ptr, new_path_len) -> errno
pub fn path_rename(
    caller: Caller<'_, RuntimeState>,
    old_fd: i32,
    old_path_ptr: i32,
    old_path_len: i32,
    new_fd: i32,
    new_path_ptr: i32,
    new_path_len: i32,
) -> Result<i32, Error> {
    let src = read_path(&caller, old_fd, old_path_ptr, old_path_len)?;
    if !caller.data().grants(&src) { return Ok(76); } // ENOTCAPABLE — src
    let dst = read_path(&caller, new_fd, new_path_ptr, new_path_len)?;
    if !caller.data().grants(&dst) { return Ok(76); } // ENOTCAPABLE — dst
    Err(Error::host(crate::wasm::suspend::SuspendReason::PathRename { src, dst }))
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("wasi_snapshot_preview1", "path_open", path_open)?
        .func_wrap("wasi_snapshot_preview1", "path_unlink_file", path_unlink_file)?
        .func_wrap("wasi_snapshot_preview1", "path_create_directory", path_create_directory)?
        .func_wrap("wasi_snapshot_preview1", "path_remove_directory", path_remove_directory)?
        .func_wrap("wasi_snapshot_preview1", "path_filestat_get", path_filestat_get)?
        .func_wrap("wasi_snapshot_preview1", "path_rename", path_rename)?;
    Ok(())
}
