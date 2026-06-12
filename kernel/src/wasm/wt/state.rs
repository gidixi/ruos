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
    /// Environ "K=V" (bytes, no NUL). Empty for classic tools; threaded modules
    /// get RAYON_NUM_THREADS injected by `threads::exec_threaded`.
    pub env: Vec<Vec<u8>>,
    pub exit: Option<i32>,
    /// fd table. 0/1/2 = Console; 3 = virtual preopen "/" (handled in the WASI
    /// fns, kept as Closed here); 4.. = files opened via path_open.
    pub fds: Vec<WtFd>,
    /// If set, stdout/stderr (fd 1/2) are written to this PTY slave VFS fd
    /// (so output reaches the bound terminal/SSH channel). None → CONSOLE.
    pub stdout_pty: Option<crate::vfs::Fd>,
    /// Thread group of the owning threaded app (MT Fase 2); None for classic
    /// single-threaded modules. Read by the "wasi" `thread-spawn` host fn.
    pub threads: Option<alloc::sync::Arc<crate::wasm::wt::threads::ThreadGroup>>,
}

impl WtState {
    pub fn new(args: Vec<Vec<u8>>) -> Self {
        Self {
            args,
            env: Vec::new(),
            exit: None,
            fds: alloc::vec![WtFd::Console, WtFd::Console, WtFd::Console, WtFd::Closed],
            stdout_pty: None,
            threads: None,
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

/// Capability accessor: any Store-data type that carries a `WtState` (the WASI
/// state) exposes it here, so `wasi::add_to_linker` can be generic over the
/// store-data type instead of hard-wired to `WtState`. `WtState` itself is the
/// trivial holder (returns `self`); `AppState` (compositor windows) returns its
/// embedded `wasi` field.
pub trait HasWasi {
    fn wasi(&mut self) -> &mut WtState;
    fn wasi_ref(&self) -> &WtState;
}

impl HasWasi for WtState {
    fn wasi(&mut self) -> &mut WtState { self }
    fn wasi_ref(&self) -> &WtState { self }
}
