# 435 — init: runner pool daemon con restart/backoff + stop cooperativo

**Data:** 2026-06-11

## Cosa
- `executor/mod.rs`: `unit_runner_task` (embassy pool_size=8 =
  `service::MAX_DAEMONS`) — loop esegui→policy: `.cwasm` su compute core
  via `exec_cwasm_inner`, `.wasm` wasmi inline BSP; restart per
  Always/OnFailure(code≠0) con `backoff_ticks`, reset del contatore se
  uptime >60s; `stop_requested` consumato all'uscita → niente riavvio.
  Dispatcher: Daemon o restart≠No → spawn runner (pool pieno →
  `Failed(noslot)`), oneshot puro → exec inline come prima.
- `wasm/fiber.rs`: factor `exec_cwasm_inner(bytes, argv, pts)` da
  `exec_cwasm_parallel` — il runner possiede il pid (serve a
  `request_kill`), la shell mantiene il comportamento attuale.

## Perché
Fase 4 spec init-units: supervisione daemon senza bloccare il dispatcher,
placement esecuzione ereditato dal routing per-formato.

## File toccati
- kernel/src/executor/mod.rs
- kernel/src/wasm/fiber.rs
