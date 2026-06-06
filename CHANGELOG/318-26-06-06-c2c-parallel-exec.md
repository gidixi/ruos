# 318 — C2c: per-request parallel .cwasm exec

**Data:** 2026-06-06

## Cosa

Implementato C2c: esecuzione parallela di app `.cwasm` su core ComputeApp distinti,
con reply per-request invece del singolo slot statico C2b.

Modifiche principali:

- **`kernel/src/executor/mod.rs`**
  - Sostituito `APP_REPLY` statico (singolo slot C2b) con `ExecReply` / `ExecReplyFuture`
    per-request (Arc condiviso tra spawner e task).
  - Aggiunto `pick_compute_core()`: round-robin su core con ruolo `ComputeApp`
    via `AtomicUsize` cursor.
  - `run_app_on_core` portato a `pool_size = 4`; riceve `Arc<ExecReply>`.
  - `exec_worker_task` ora gestisce solo `compositor.cwasm` + `.wasm` (wasmi).
  - Boot-check gate: task `parallel_probe` (`pool_size = 2`), loop di calcolo puro
    (200M op multiply-xor-shift × 3 iter), misura tempi con `timer::ticks()` (100 Hz).

- **`kernel/src/wasm/fiber.rs`**
  - Aggiunta `exec_cwasm_parallel`: legge il file `.cwasm`, sceglie un core via
    `pick_compute_core()`, fa `spawn_on()` con `Arc<ExecReply>`, poi `await` sul future.
    Fallback inline se nessun core disponibile.
  - `SuspendReason::Exec` handler: `.cwasm` non-compositor → `exec_cwasm_parallel`;
    resto (`.wasm`, `compositor.cwasm`) → `EXEC_QUEUE` invariato.

- **`kernel/src/boot/phases/interrupts.rs`**
  - Boot-check C2c: fase 0 (baseline singolo) + fase 1 (due probe concorrenti).
  - Log gate: `parallel-exec ran=[c0,c1] concurrent_ms=X single_ms=Y overlap=Z`.
  - Condizione overlap: `concurrent_ms ≤ single_ms × 1.6`.

- **`kernel/src/wasm/wt/mod.rs`**
  - Aggiunto `RUN_CWASM_LOCK: spin::Mutex<()>` per serializzare chiamate concorrenti
    a `run_cwasm()`. Wasmtime no_std `RwLock` panica su write concorrenti; il mutex
    protegge sia il path di produzione sia futuri usi multi-core.

## Perché

Il singolo slot `APP_REPLY` statico (C2b) corrompe il risultato se due shell
eseguono `.cwasm` in parallelo: la seconda sovrascrive il codice di uscita prima
che la prima lo legga. C2c risolve con `Arc<ExecReply>` per ogni richiesta e
distribuisce i task su core diversi, sfruttando l'SMP già presente nel kernel.

Gate parallelismo verificato con `make iso CARGO_FEATURES=boot-checks`:
`parallel-exec ran=[2,3] concurrent_ms=1010 single_ms=1010 overlap=true`

`make run-test` rimane verde.

## File toccati

- kernel/src/executor/mod.rs
- kernel/src/wasm/fiber.rs
- kernel/src/boot/phases/interrupts.rs
- kernel/src/wasm/wt/mod.rs
- CHANGELOG/318-26-06-06-c2c-parallel-exec.md
