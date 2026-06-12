# 489 — MT Fase 2 Task 3: exec_threaded — il main dei moduli threaded gira su fiber

**Data:** 2026-06-12

## Cosa

- **Route in `run_cwasm`** (`wt/mod.rs`): un modulo che importa `env::memory`
  *shared* (firma dei `wasm32-wasip1-threads`) viene deviato su
  `threads::exec_threaded` — il suo `_start` DEVE girare dentro un fiber,
  altrimenti un futuro `atomic.wait` nel main non avrebbe nulla da sospendere.
- **`exec_threaded`** (`wt/threads.rs`): crea `SharedMemory` dal tipo
  dell'import + Linker condiviso del gruppo (wasi + gfx + gui + thread-spawn)
  con `env::memory` definita UNA volta (SharedMemory è engine-scoped, vale per
  ogni Store del gruppo); `ThreadGroup` con env `RAYON_NUM_THREADS=<n core
  ComputeApp>`; main = fiber tid 0 via `spawn_fiber`; attesa cooperativa di
  `live==0` **drenando i fiber dal proprio core** (senza, su un sistema con un
  solo core abilitato il main non girerebbe mai) + `hlt` quando idle, sveglia
  via `waiter_core` (nuovo campo, `finish_fiber` ora sveglia quel core, non
  più il BSP fisso).
- **`spawn_fiber`**: corpo del fiber = Store+WtState (env+threads handle, PTY
  solo per il main), instantiate dal linker del gruppo, `cld`, `_start`
  (tid 0) o `wasi_thread_start(tid, start_arg)` (tid>0, dal Task 5), exit
  code da return/proc_exit/trap (trap → `poisoned`, kill-group nel Task 7).
- **`add_thread_spawn_to_linker`**: import `("wasi", "thread-spawn")`
  registrato (i moduli wasip1-threads lo importano sempre → serve già per
  l'instantiate); per ora STUB che ritorna -1 — lo spawn reale è il Task 5.
- **`WtState.env` + `WtState.threads`** (`state.rs`) e **environ WASI reali**
  (`wasi.rs`): `environ_sizes_get`/`environ_get` implementati col pattern di
  `args_*` su `WtState.env` (prima stub a 0 — i tool classici restano a env
  vuoto, identico comportamento).

## Perché

MT Fase 2 Task 3 (piano `docs/superpowers/plans/2026-06-12-wasm-mt-fase2-threads.md`):
posa il path di esecuzione dei moduli threaded (gruppo + memoria condivisa +
main su fiber) su cui i Task 4 (futex), 5 (thread-spawn) e 6 (rayon) si
appoggiano. L'env injection serve a rayon, che senza
`available_parallelism` legge `RAYON_NUM_THREADS` dall'environ.

## File toccati

- kernel/src/wasm/wt/threads.rs
- kernel/src/wasm/wt/mod.rs
- kernel/src/wasm/wt/state.rs
- kernel/src/wasm/wt/wasi.rs
- docs/superpowers/plans/2026-06-12-wasm-mt-fase2-threads.md
- docs/superpowers/plans/2026-06-12-wasm-mt-fase2-task2-fiber-runtime.md
- CHANGELOG/489-26-06-12-wt-exec-threaded.md
