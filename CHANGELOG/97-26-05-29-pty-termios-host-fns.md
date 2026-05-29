# 97 — WASIX termios host fns (T4)

**Data:** 2026-05-29

## Cosa

- `kernel/src/wasm/host/term.rs` (nuovo): host fns sotto module "ruos":
  - `tcgetattr(fd, ptr)` → scrive Termios di pair[idx] in wasm memory
  - `tcsetattr(fd, action, ptr)` → legge Termios da wasm memory e
    sostituisce in pair[idx]. action ignored (sempre TCSANOW).
  - `isatty(fd)` → 1 se FD backs PtySlaveFile, 0 altrimenti.
- `fd_to_pty` helper: walk `RuntimeState.fds` → `vfs::FDS` → FileImpl
  variant per recuperare PTY idx. Ritorna ENOTTY (25) per FD non-PTY.
- `kernel/src/wasm/host/mod.rs`: aggiunto `pub mod term;` + link.

## Perché

Step 12 T4. T5 shell.wasm chiamerà tcsetattr per droppare ICANON/
ECHO/ISIG e entrare in raw mode per il line editor.

## File toccati

- kernel/src/wasm/host/term.rs (nuovo)
- kernel/src/wasm/host/mod.rs
- CHANGELOG/97-26-05-29-pty-termios-host-fns.md (nuovo)
