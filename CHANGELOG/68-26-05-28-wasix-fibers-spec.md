# 68 — Spec design: WASIX fibers (Step 10.5)

**Data:** 2026-05-28

## Cosa

Scritta spec Step 10.5 in
`docs/superpowers/specs/2026-05-28-rust-wasix-fibers-design.md`.

Architettura green-threads / fiber pattern:
- Fiber = istanza wasmi
- `wasmi::Func::call_resumable` cattura continuation
- Host fns I/O ritornano `Err(SuspendReason::*)` invece di `block_on`
- Loop esterno `Fiber::run` (async) decodifica SuspendReason → await
  future → `state.resume(...)`
- Embassy executor = multiplexer

Decisioni strategiche (brainstorm):
- **Opt A migration scope**: tutti I/O host fns (sock_*, fd_*, path_*,
  kbd) a SuspendReason. Lifecycle (args/proc_exit) restano sync.
- Smoke contract finale: stesso `client.wasm: rx='pong'` ma vero
  (senza pre-loading).
- Cleanup integrato: drop `setup_demo_sockets`, drop sync wrappers.

Decomposizione 3 task:
1. Fiber scaffolding + `SuspendReason::Sleep` (warmup).
2. Migrate sock_* + drop pre-loading; verifica vero roundtrip TCP.
3. Migrate fd_*/path_*/kbd → cleanup finale.

Pattern critico documentato: "dati in memory, errno on resume" —
le host fns che scrivono buffer in wasm memory (sock_recv ecc.)
includono i pointer nel SuspendReason; il loop esterno usa
`instance.get_export("memory")` per scrivere via store.

Out of scope: lifecycle host fns sync, drop embassy_futures,
followup F4-F7 di Step 10.

## Perché

F-MAJOR di `docs/followups/step-10.md`. Risolve il limite
`embassy_futures::block_on` (busy-poll, no yield) che ha forzato
`setup_demo_sockets` a Task 6. Sblocca real cooperative wasm async.

## File toccati

- docs/superpowers/specs/2026-05-28-rust-wasix-fibers-design.md (nuovo)
- CHANGELOG/68-26-05-28-wasix-fibers-spec.md (nuovo)
