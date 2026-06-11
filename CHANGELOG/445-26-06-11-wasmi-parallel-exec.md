# 445 — Exec `.wasm` (wasmi) parallelo su compute core

**Data:** 2026-06-11

## Cosa
I tool `.wasm` (wasmi) ora girano su un ComputeApp core dedicato, in parallelo,
invece di passare tutti per l'unico `exec_worker_task` globale.

- `kernel/src/executor/mod.rs`: nuovo task `run_wasmi_on_core` (pool_size=4),
  gemello wasmi di `run_app_on_core`. Costruisce il `Fiber` SUL core target
  (così solo `bytes`/`argv`/`cwd`/`name`/`Arc<ExecReply>` — tutti `Send` —
  attraversano il confine) e replica il setup interattivo del ramo `.wasm` di
  `exec_worker_task` (rebind PTY, `proc::register`/`set_pid`, termios cooked +
  foreground pid, restore a fine run), poi `reply.complete(code)`.
- `kernel/src/wasm/fiber.rs`: nuovo `exec_wasmi_parallel` (gemello di
  `exec_cwasm_parallel`) + reroute in `dispatch(Exec)`: i `.wasm` non-speciali
  vanno al path parallelo. Fallback: nessun ComputeApp core (≤2 CPU) →
  `EXEC_QUEUE` single-slot; pool pieno → 127 (come il path cwasm).
  `compositor.cwasm` resta su `EXEC_QUEUE` (hand-off al GUI core).

## Perché
Un `.wasm` interattivo/long-running (es. `rtop`) occupava l'unico
`exec_worker_task` per tutta la sua vita: il worker non tornava mai a
`WaitForRequest`, quindi ogni comando lanciato in un altro terminale restava
in coda su `EXEC_QUEUE` e non partiva finché rtop non usciva ("rtop blocca
tutti gli altri terminali"). Il path `.cwasm` era già parallelo (per-request
`Arc<ExecReply>` + ComputeApp core); il path wasmi era rimasto single-slot.

Verificato che lo `Store` wasmi è `Send` (il future di `run_wasmi_on_core`
passa `spawn_on`, `cargo check` pulito) — a differenza del path wasmtime che
resta sincrono via `run_cwasm`.

## File toccati
- kernel/src/executor/mod.rs
- kernel/src/wasm/fiber.rs
- kernel/src/wasm/exec_queue.rs (doc-comment di modulo allineato allo scope ridotto)
- CHANGELOG/444-26-06-11-wasmi-parallel-exec.md
