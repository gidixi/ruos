# 491 — MT Fase 2 Task 5: thread-spawn reale + Gate 2 + threads-test

**Data:** 2026-06-12

## Cosa

- **`("wasi", "thread-spawn")` reale** (`wt/threads.rs`,
  `add_thread_spawn_to_linker`): lo stub -1 del Task 3 diventa lo spawn vero —
  tid da `next_tid` (range `[1, 2^29)`), rifiuto se gruppo `poisoned`,
  `spawn_fiber(g, tid, start_arg, None)`: nuovo fiber accodato runnable (lo
  prende il primo core libero, NON esegue inline) con fresh Instance dello
  stesso Module sulla STESSA SharedMemory, entry `wasi_thread_start(tid,
  start_arg)`. Stack e TLS del thread sono affare del guest (preparati da
  `pthread_create` nel blocco `start_arg`). Ritorno: tid, o -1 → `EAGAIN`
  guest-side.
- **`spawn_fiber_export`** (path di test): il WtState dei fiber gate ora porta
  il `threads` handle — serviva al main del gate 2 per chiamare thread-spawn.
- **Gate 2** (`THREADS-OK 2`): `tools/wt-threads-gate/gate2.wat` — il main
  (export `run`) spawna un thread via l'import, attende `atomic.wait` su
  mem[64], il thread (`wasi_thread_start`) scrive 99 + notify, il main
  rilegge. Prova la catena: spawn → fresh Instance → stessa memoria → il main
  vede la scrittura del child. Runner `gate2_run` + regola Makefile + include
  + chiamata boot.
- **`tests/threads-test.sh`** + target **`make run-threads-test`**: builda la
  ISO boot-checks e asserisce i 4 marker (`THREADS-OK 1/2/3` +
  `THREADS-FIBER-OK`) su `-smp 4` E su `-smp 1` (regressione deadlock del
  fallback BSP).
- `docs/api/wasi.md`: entry thread-spawn aggiornata (da stub a semantica
  reale, stesso commit — regola docs/api).

## Perché

MT Fase 2 Task 5: con lo spawn reale `pthread_create`/`std::thread::spawn`
dei moduli `wasm32-wasip1-threads` diventano funzionanti — era l'ultimo
pezzo runtime mancante prima del test end-to-end con rayon (Task 6).

## Verifica

- `make run-threads-test`: `TEST_PASS_THREADS` (tutti i marker ok su -smp 4 e
  -smp 1).
- `make run-test`: `TEST_PASS`.

## File toccati

- kernel/src/wasm/wt/threads.rs
- kernel/src/wasm/wt/mod.rs
- kernel/src/boot/phases/interrupts.rs
- tools/wt-threads-gate/gate2.wat (nuovo)
- tests/threads-test.sh (nuovo)
- Makefile
- docs/api/wasi.md
- docs/superpowers/plans/2026-06-12-wasm-mt-fase2-threads.md
- docs/superpowers/plans/2026-06-12-wasm-mt-fase2-task2-fiber-runtime.md
- CHANGELOG/491-26-06-12-wt-thread-spawn-gate2.md
