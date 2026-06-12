# 490 — MT Fase 2 Task 4: hook futex (atomic.wait/notify) + Gate 3

**Data:** 2026-06-12

## Cosa

- **Hook futex reali** in `wt/threads.rs`: `wasmtime_futex_wait32/64` =
  spin adattivo (200 iter PAUSE) → ricontrollo del valore sotto il lock dello
  shard (serializza col percorso notify; la finestra park-in-volo resta coperta
  dai *crediti* del Task 2) → `park_current` (suspend del fiber, il core resta
  libero); ritorno 0=woken / 1=not-equal / 2=timed-out, `timeout_ns<0` =
  infinito (ns→tick 100 Hz arrotondati in su). `wasmtime_futex_notify` =
  `wake_key` (dequeue dallo shard → RUNQ → IPI broadcast). Contesto non-fiber
  → degradazione a spin con warn (non deve succedere, non deve deadlockare).
  **Rimossi gli stub** del Task 0 da `wt/platform.rs` (collisione `#[no_mangle]`).
- **`expire_timeouts()`**: riscatto dei waiter a deadline scaduta → RUNQ
  (il wait ritorna 2). Pre-filtro O(1) `TIMED_WAITERS` (contatore dei soli
  waiter CON timeout, mantenuto sotto i lock shard) al posto del pre-filtro
  `EARLIEST_DEADLINE` suggerito dal piano: l'atomic min-tracking aveva una
  race insert-vs-rescan (deadline persa ⇒ oversleep indefinito), il contatore
  no. Chiamato dal seam di `run_core` e dal wait-loop di `exec_threaded`
  (su 1-2 core nessun `run_core` gira mentre quello blocca il core).
- **Gate 3** (`THREADS-OK 3`): `tools/wt-threads-gate/gate3.wat` — `waiter`
  fa `memory.atomic.wait32` infinito, `waker` scrive il payload e notify;
  runner `gate3_run` (boot-checks) li spawna su DUE fiber dello stesso gruppo
  via `spawn_fiber_export` (variante test di spawn_fiber: export custom
  `()->i32`), assert exit waiter = 7. Prova che il wait sospende il FIBER
  (il waker gira anche con un solo core abilitato) e che notify risveglia.
  Regola Makefile `threads_gate3.cwasm` + include + chiamata boot.

## Perché

MT Fase 2 Task 4: `atomic.wait` deve cedere il core (punto 3 del gate della
spec) — è il pezzo che rende possibile `pthread_mutex`/`condvar`/`join` dei
moduli `wasm32-wasip1-threads` senza bruciare un core per ogni thread bloccato.

## Verifica

- QEMU `-smp 4`: `THREADS-OK 1 = ok`, `THREADS-FIBER-OK = ok ran_on=2`,
  `THREADS-OK 3 = ok`.
- QEMU `-smp 2` e `-smp 1` (fallback BSP, nessun ComputeApp): `THREADS-OK 3 =
  ok`, nessun deadlock.
- `make run-test`: `TEST_PASS`.

## File toccati

- kernel/src/wasm/wt/threads.rs
- kernel/src/wasm/wt/platform.rs
- kernel/src/wasm/wt/mod.rs
- kernel/src/executor/mod.rs
- kernel/src/boot/phases/interrupts.rs
- tools/wt-threads-gate/gate3.wat (nuovo)
- Makefile
- docs/superpowers/plans/2026-06-12-wasm-mt-fase2-threads.md
- docs/superpowers/plans/2026-06-12-wasm-mt-fase2-task2-fiber-runtime.md
- CHANGELOG/490-26-06-12-wt-futex-hooks-gate3.md
