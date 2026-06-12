# 478 — MT Fase 2: design wasm-threads (fiber cooperativi M:N)

**Data:** 2026-06-12

## Cosa

Design spec di Fase 2 (wasm-threads MVP) — `std::thread`/rayon nelle app
`.cwasm` (`wasm32-wasip1-threads`). Modello scelto: **thread = fiber cooperativi
schedulati M:N** sui core ComputeApp (cedono solo a `atomic.wait`/host-call/
return, non preemptive). Punti chiave:

- Bring-up gate a 3 punti come prerequisito hard (atomics+SharedMemory,
  `wasi_thread_spawn`, `atomic.wait` sospende un fiber via `async_support`).
- `atomic.wait` = adaptive spin (`PAUSE`) → suspend fiber; `atomic.notify` = IPI.
  Atomics non contesi = nativi x86 `lock`-prefixed (Cranelift, zero host-call).
- run_core esteso a work-stealing su {job compositing, task executor, fiber
  thread}; il compositing degrada via fallback inline di Fase 1.
- Stato scheduler per-core `#[repr(align(64))]` (no false sharing); `likely`/
  `#[cold]` sugli hot loop.
- Niente fallback al modello pinnato: se il gate cade si risolve `async_support`
  in no_std.

## Perché

Fase 2 della roadmap MT. L'outline va rispecificato a Fase 1 conclusa (changelog
476). Scelta del modello fatta in brainstorming: fiber M:N batte i core pinnati
(thread parcheggiato costa zero core, oversubscription cooperativa gratis,
parallelismo vero fino a num_core, on-pivot perché cooperativo non preemptive).

## File toccati

- docs/superpowers/specs/2026-06-12-wasm-mt-fase2-threads-design.md (nuovo)
