//! Per-instance state for a Wasmtime guest: argv, exit code, and an fd table.
//! Backed by the kernel VFS (synchronous via `crate::vfs::block_on`).

use alloc::vec::Vec;

pub enum WtFd {
    /// stdio (0/1/2) → CONSOLE for write, EOF for read.
    Console,
    /// An open VFS file descriptor.
    Vfs(crate::vfs::Fd),
    Closed,
}

pub struct WtState {
    pub args: Vec<Vec<u8>>,
    pub exit: Option<i32>,
    /// fd table. 0/1/2 = Console; 3 = virtual preopen "/" (handled in the WASI
    /// fns, kept as Closed here); 4.. = files opened via path_open.
    pub fds: Vec<WtFd>,
}

impl WtState {
    pub fn new(args: Vec<Vec<u8>>) -> Self {
        Self {
            args,
            exit: None,
            fds: alloc::vec![WtFd::Console, WtFd::Console, WtFd::Console, WtFd::Closed],
        }
    }

    /// Install an open VFS fd, returning the guest fd number.
    pub fn install_vfs(&mut self, f: crate::vfs::Fd) -> i32 {
        self.fds.push(WtFd::Vfs(f));
        (self.fds.len() - 1) as i32
    }

    pub fn get(&self, fd: i32) -> Option<&WtFd> {
        if fd < 0 { return None; }
        self.fds.get(fd as usize)
    }
}
