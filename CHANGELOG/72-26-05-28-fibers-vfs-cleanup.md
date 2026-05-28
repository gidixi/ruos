# 72 — Step 10.5 Task 3: migrate fd_*/path_*/kbd to SuspendReason; drop Runtime

**Data:** 2026-05-28

## Cosa

All remaining wasm host fns that perform I/O now trap with `SuspendReason`
instead of calling `embassy_futures::block_on`:

- `fd_read` (Stdin → `KbdReadChar`, Vfs → `VfsRead`)
- `fd_write` (Vfs → `VfsWrite`; Socket already T2; Console still synchronous)
- `fd_seek` (Vfs → `VfsSeek`)
- `fd_close` (Vfs → `VfsClose`)
- `path_open` (→ `PathOpen`; FD allocation moved to `Fiber::dispatch`)

`Fiber::dispatch` extended with matching arms for all new variants.

Dead `Runtime` struct (sync `.run()` + default Engine) deleted from
`wasm/mod.rs`. No `embassy_futures::block_on` call remains in any host fn.

## Perché

Step 10.5 goal: cooperative green-threads via `Func::call_resumable`.
All blocking in host fns must yield to the async executor, not busy-wait.
After T3 the pattern is fully wired across sock_*, fd_*, path_*, kbd.

## File toccati

- `kernel/src/wasm/host/fd.rs`
- `kernel/src/wasm/host/path.rs`
- `kernel/src/wasm/fiber.rs`
- `kernel/src/wasm/mod.rs`
- `CHANGELOG/72-26-05-28-fibers-vfs-cleanup.md`
