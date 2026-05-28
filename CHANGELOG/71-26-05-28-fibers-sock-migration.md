# 71 — Fibers: sock_* via SuspendReason; drop pre-loading

**Data:** 2026-05-28

## Cosa
- `sock_accept` host fn traps with `SuspendReason::SockAccept`; `Fiber::run` awaits
  `crate::net::sockets::accept()` cooperatively.
- `fd_write` and `fd_read` Socket arms trap with `SuspendReason::SockSend` /
  `SuspendReason::SockRecv`; fiber awaits `send()` / `recv()` futures.
- `Fiber::dispatch` extended with `SockAccept`, `SockConnect`, `SockRecv`,
  `SockSend` arms.
- `find_fd_for_handle` helper added to `Fiber`.
- `setup_demo_sockets()`, `SERVER_SOCK_IDX`, `CLIENT_SOCK_IDX` deleted from
  `wasm/mod.rs`.
- `connect_sync`, `accept_sync`, `recv_sync`, `send_sync` deleted from
  `net/sockets.rs`; replaced by async `connect`, `accept`, `recv`, `send`.
- `run_at()` now allocates and (for client) async-connects sockets before
  spawning each wasm fiber.
- Executor spawns `/server.wasm` and `/client.wasm` tasks alongside `/init.wasm`.
- `kmain` emits `ruos: real ping-pong (no preload)` after `net::init()` as
  architectural assertion.
- Makefile `HELLO` sentinel bumped to `ruos: real ping-pong (no preload)`.

## Perché
Step 10.5 Task 2: replace synchronous spin-poll socket wrappers with cooperative
async futures driven by the embassy executor. The ping/pong exchange now happens
via real cooperative TCP roundtrip: server.wasm and client.wasm yield via
SuspendReason, the executor drives both fibers and the net_poll_task, delivering
bytes through smoltcp without ever blocking the CPU in a spin loop.

## File toccati
- `kernel/src/wasm/host/sock.rs`
- `kernel/src/wasm/host/fd.rs`
- `kernel/src/wasm/fiber.rs`
- `kernel/src/wasm/mod.rs`
- `kernel/src/net/sockets.rs`
- `kernel/src/main.rs`
- `kernel/src/executor/mod.rs`
- `Makefile`
- `CHANGELOG/71-26-05-28-fibers-sock-migration.md`
