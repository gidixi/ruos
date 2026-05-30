//! Helper for spawning a wasm program tied to a specific PTY index.
//!
//! Used by the SSH server: when a client opens an interactive shell session,
//! we allocate a PTY pair and then spawn `shell.wasm` with FDs 0/1/2 bound
//! to that pair's slave instead of the default `/dev/pts/0`.

use alloc::string::String;

/// Spawn `shell.wasm` on `/dev/pts/<idx>`. Drops into the existing wasm task
/// pool — same execution model as the boot shell, just on a different PTY.
pub fn spawn_shell_on_pty(idx: usize) {
    crate::executor::enqueue_shell_pty(idx, String::from("/bin/shell.wasm"));
}
