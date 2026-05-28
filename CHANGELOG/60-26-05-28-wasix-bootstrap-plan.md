# 60 — Piano implementazione: WASIX bootstrap (Step 10)

**Data:** 2026-05-28

## Cosa

Scritto piano Step 10 in
`docs/superpowers/plans/2026-05-28-rust-wasix-bootstrap.md`. Sei task
TDD bite-sized:

1. **Limine modules → VFS mount**: `limine.conf` + `modules.rs`,
   placeholder 1-byte `init.wasm`. HELLO: `mounted 1 boot modules`.
2. **wasmi up + lifecycle + fd_write console**: deps wasmi 0.36 +
   embassy-futures, `wasm/` modulo, `RuntimeState` + 5 host fns +
   `wasm_task` embassy. Demo: `init.wasm` welcome banner. HELLO:
   `init.wasm exited cleanly`.
3. **VFS-backed fd_* + path_***: `path_open` → VFS, `fd_read/seek/close`
   reali, dispatch socket-aware in `fd_write`. init.wasm aggiunge
   smoke `open /dev/null + write`. HELLO: `init.wasm: vfs smoke ok`.
4. **clock + random + stdin**: `clock_time_get`, `random_get` (xorshift
   weak), FD 0 → keyboard queue. init.wasm stampa uptime+rand.
   HELLO: `init.wasm: clock_rand ok`.
5. **smoltcp + Loopback**: dep smoltcp 0.11, `net/` modulo, Loopback
   device + Interface + SocketSet, `net_poll_task` 10ms. HELLO:
   `net init ok addr=127.0.0.1/8`.
6. **sock_* host fns + demo ping/pong**: `SockPool`, async accept/
   connect/recv/send, `sock_*` host fns, server.wasm + client.wasm
   come moduli Limine separati. HELLO finale: `client.wasm: rx='pong'`.

TDD kernel-style: HELLO sentinel cambia per task. Numerazione changelog
implementer: 61-66.

Pre-flight nel piano: `rustup target add wasm32-wasip1`.

## Perché

Step 10 monolitico per scelta utente (Opt B brainstorm). Sei task
con checkpoints intermedi danno granularità di review/rollback senza
allungare a 2 step separati. Subagent-driven pronto.

## File toccati

- docs/superpowers/plans/2026-05-28-rust-wasix-bootstrap.md (nuovo)
- CHANGELOG/60-26-05-28-wasix-bootstrap-plan.md (nuovo)
