# 69 — Piano implementazione: WASIX fibers (Step 10.5)

**Data:** 2026-05-28

## Cosa

Scritto piano Step 10.5 in
`docs/superpowers/plans/2026-05-28-rust-wasix-fibers.md`. Tre task
TDD bite-sized:

1. **Fiber scaffolding + Sleep via poll_oneoff**: nuovi
   `wasm/{fiber,suspend}.rs`. `Fiber::run` async + `call_resumable`
   pattern. `poll_oneoff` subset (clock only) ritorna
   `SuspendReason::Sleep`. init.wasm aggiunge `thread::sleep(500ms)`.
   HELLO: `init.wasm: slept ok`. Visual proof: `async tick=N`
   interleaved nel sleep window.
2. **Migrate sock_* + drop setup_demo_sockets**: sock_accept/connect
   + fd_read/write Socket arms a SuspendReason. Drop
   `setup_demo_sockets` + sync wrappers + statics. Add
   asserzione architetturale `ruos: real ping-pong (no preload)`.
   HELLO: `ruos: real ping-pong (no preload)`.
3. **Migrate fd_*/path_*/kbd + cleanup**: tutto restante I/O a
   SuspendReason. Drop `Runtime` struct. HELLO invariato.

Architettura: pattern "dati in memory, errno on resume" documentato.
Host fns scrivono buf ptr in SuspendReason; Fiber::run usa
`instance.get_export("memory")` per scrivere via store.

Risk: wasmi 1.0.9 `Func::call_resumable` API signature, fallback
documentati. Multi-iov socket/vfs writes = EINVAL accettabile.

## Perché

Tradurre lo spec Step 10.5 in passi eseguibili TDD. Sleep prima
(scaffolding), sock_* dopo (core), fd_*/path_* per ultimo (cleanup).
Subagent-driven pronto.

## File toccati

- docs/superpowers/plans/2026-05-28-rust-wasix-fibers.md (nuovo)
- CHANGELOG/69-26-05-28-wasix-fibers-plan.md (nuovo)
