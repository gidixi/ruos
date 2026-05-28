//! Per-instance runtime state for a wasm task.

use alloc::vec::Vec;
use core::sync::atomic::AtomicI32;

pub struct RuntimeState {
    /// File descriptor table: index = FD, value = `FdEntry` (or None).
    /// FDs 0/1/2 are reserved for stdin/stdout/stderr.
    pub fds: Vec<Option<FdEntry>>,
    pub args: Vec<Vec<u8>>,
    pub env: Vec<Vec<u8>>,
    pub exit_code: AtomicI32,
}

pub enum FdEntry {
    /// Special: writes go to the console. FD 1 and 2 use this in Task 2.
    StdoutConsole,
    /// VFS-backed file (populated in Task 3).
    Vfs(crate::vfs::Fd),
}

impl RuntimeState {
    pub fn new() -> Self {
        let mut fds: Vec<Option<FdEntry>> = (0..16).map(|_| None).collect();
        fds[0] = None; // stdin: not connected
        fds[1] = Some(FdEntry::StdoutConsole); // stdout → console
        fds[2] = Some(FdEntry::StdoutConsole); // stderr → console
        Self {
            fds,
            args: Vec::new(),
            env: Vec::new(),
            exit_code: AtomicI32::new(0),
        }
    }
}
