//! Per-instance runtime state for a wasm task.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::AtomicI32;

pub struct RuntimeState {
    /// File descriptor table: index = FD, value = `FdEntry` (or None).
    /// FDs 0/1/2 are backed by /dev/pts/0 (PTY slave) after Task 3.
    pub fds: Vec<Option<FdEntry>>,
    pub args: Vec<Vec<u8>>,
    pub env: Vec<Vec<u8>>,
    pub exit_code: AtomicI32,
    /// Current working directory. Relative paths in `path_open` are
    /// resolved against this. New Fibers default to "/"; children
    /// spawned via `ruos_exec` inherit the parent's CWD.
    pub cwd: String,
}

pub enum FdEntry {
    /// Special: writes go to the console directly (legacy fallback).
    StdoutConsole,
    /// VFS-backed file (includes /dev/pts/0 for FD 0/1/2).
    Vfs(crate::vfs::Fd),
    /// Kernel TCP socket (index into net::sockets::POOL).
    Socket(usize),
}

impl RuntimeState {
    pub fn new() -> Self {
        let mut fds: Vec<Option<FdEntry>> = (0..16).map(|_| None).collect();
        use crate::vfs;
        // Open /dev/pts/0 thrice for FD 0/1/2.
        // tmpfs open completes in a single poll, so block_on works fine here.
        // RuntimeState::new is only called from Fiber::new, which is invoked
        // from wasm_task — by that time vfs::init has long since run.
        for slot in 0..3 {
            match vfs::block_on(vfs::open(
                "/dev/pts/0",
                vfs::OpenFlags::READ | vfs::OpenFlags::WRITE,
            )) {
                Ok(fd) => { fds[slot] = Some(FdEntry::Vfs(fd)); }
                Err(_) => {} // leave None; wasm may fail to read/write
            }
        }
        Self {
            fds,
            args: Vec::new(),
            env: Vec::new(),
            exit_code: AtomicI32::new(0),
            cwd: String::from("/"),
        }
    }
}
