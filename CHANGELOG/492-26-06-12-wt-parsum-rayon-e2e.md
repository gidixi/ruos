# 492 — MT Fase 2 Task 6: parsum — std::thread + rayon end-to-end + ps tids

**Data:** 2026-06-12

## Cosa

- **`tools/parsum/`** (nuovo): primo binario Rust std reale su
  `wasm32-wasip1-threads` — somma parallela con **rayon**, stampa
  `PARSUM_OK threads=N sum=S speedup_x100=X`. `threads=N` prova che
  `RAYON_NUM_THREADS` (iniettato da `exec_threaded` nell'environ) arriva al
  guest; `sum` identica tra serial e parallel prova la coerenza della
  SharedMemory tra i fiber.
- **Makefile**: regola `build/parsum.cwasm` (cargo `--target
  wasm32-wasip1-threads` + wt-precompile), dipendenza del target `iso`,
  staging in `/bin` (binstage). Target rustup aggiunto a `build-iso.ps1`.
- **ps tids**: `ThreadGroup.base_name` (argv[0]) + `ThreadFiber.pid` — i
  thread spawnati si registrano in `proc` come `nome#tid` (`spawn_fiber`,
  solo tid>0: il main è già il processo registrato dalla shell);
  `finish_fiber` li deregistra. `ps` durante un parsum mostra `parsum#1..N`.
- **`tests/threads-test.sh` stage 2**: ISO con `user-bin/threads-init.sh`
  (nuovo), boot `-smp 6`, assert `PARSUM_OK threads=[2-9]`.

## TRE bug reali scovati da parsum (il primo binario std threaded vero)

1. **Stack fiber no_std con TOP disallineato** → `KERNEL PANIC` nell'unwinder
   (`assert_fp_is_aligned: left 8 right 0`) al PRIMO trap dentro un fiber
   (proc_exit di wasi-libc — i gate non trappano mai, per questo passavano).
   Il backend no_std di `wasmtime-internal-fiber` alloca lo stack su heap e
   allinea la BASE a 16 ma non il TOP — e lo stack cresce dal top (su Unix
   mmap è page-aligned gratis). Fix: vendor **`third_party/wasmtime-fiber45`**
   (`[patch.crates-io]`) con una riga in `align_ptr` (nostd.rs): `new_len`
   arrotondato in GIÙ all'allineamento.
2. **Accessor memoria `wt/mem.rs` cieco alle SharedMemory**: i moduli
   threaded ri-esportano la memoria IMPORTATA, quindi l'export "memory" è
   `Extern::SharedMemory`, non `Extern::Memory` → ogni WASI fn falliva EINVAL
   → wasi-libc usciva 71 (EX_OSERR) prima del main. Fix: `GuestMem`
   Plain/Shared, accessi shared = byte copy bounds-checked su
   `SharedMemory::data()` (la race col guest è il modello shared-memory,
   come upstream wasmtime-wasi).
3. **Il gruppo non moriva mai col main**: i worker rayon restano parcheggiati
   per sempre in attesa di lavoro → `live` mai 0 → `exec_threaded` non
   ritornava (core occupato all'infinito). Fix: semantica processo (come
   wasmtime-wasi-threads upstream / Linux) — quando il MAIN esce, il gruppo
   viene avvelenato e i waiter uccisi (`kill_group_waiters`); exit = quello
   del main. In più: `heartbeat_bump` nel wait-loop (il supervisor credeva
   muto il core dell'app).

In più: il probe `manifest()` del launcher ora SKIPPA i moduli threaded
(import `env::memory` shared) — sono tool CLI, non app finestra; il linker
del launcher non definisce la shared memory e ogni catalog scan (~1 Hz)
loggava un warn di instantiate fallito per ciascun tool threaded in /bin.

## Perché

MT Fase 2 Task 6: la prova end-to-end che `std::thread`/rayon funzionano
davvero nelle app `.cwasm` — toolchain guest → spawn → futex → scheduler
fiber → multi-core, dalla shell.

## File toccati

- tools/parsum/ (nuovo)
- third_party/wasmtime-fiber45/ (nuovo vendor, fix align)
- user-bin/threads-init.sh (nuovo)
- kernel/src/wasm/wt/threads.rs
- kernel/src/wasm/wt/mem.rs
- kernel/src/wasm/wt/wm.rs (probe skip threaded)
- kernel/Cargo.toml + Cargo.lock (patch wasmtime-internal-fiber)
- Makefile
- build-iso.ps1
- tests/threads-test.sh
- CHANGELOG/492-26-06-12-wt-parsum-rayon-e2e.md
