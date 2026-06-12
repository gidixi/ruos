# 484 — MT Fase 2: piano di implementazione wasm-threads

**Data:** 2026-06-12

## Cosa

Piano di implementazione della Fase 2 (spec changelog 478), fondato su una
ricerca approfondita del codebase + sorgenti wasmtime 45. Scoperte chiave che
fissano il COME:

- wasmtime 45 feature `threads` richiede hard `std` (parking_spot =
  `std::thread::park`; SharedMemory = `std::sync::RwLock`+`Instant`) → il
  piano prevede un **fork minimale vendorizzato** (`third_party/wasmtime45/`,
  `[patch.crates-io]`): feature senza std, SharedMemory su sync interno,
  libcall `memory_atomic_{wait32,wait64,notify}` → hook `extern "C"`
  `wasmtime_futex_*` implementati dal kernel.
- `wasmtime-internal-fiber` 45 ha un backend no_std con stack-switch reale →
  i fiber dei thread sono nostri, wasmtime resta sync (**niente
  `async_support`** — deviazione documentata dalla spec, stesso requisito).
- ABI wasi-threads verificata: import `("wasi","thread-spawn")`, export
  `wasi_thread_start(tid,arg)`, memoria IMPORTATA `env::memory` shared
  (l'host crea la SharedMemory), TLS tutto guest-side, rayon richiede
  `RAYON_NUM_THREADS` nell'environ (da iniettare; oggi il wt runtime ha
  environ stub vuoti).
- Thread store a `NO_DEADLINE` (precedente CLI tools) — seconda deviazione
  documentata dalla spec §7.

8 task: fork+toolchain → gate1 atomics/SharedMemory → fiber runtime →
exec_threaded → futex+gate3 → thread-spawn+gate2 → parsum (rayon) + ps →
mtstress + kill-group + regressione + VBox + docs.

## Perché

Fase 2 della roadmap MT: spec approvata (478) → ciclo spec→piano→impl.
La ricerca pre-piano evita di scoprire il muro `threads`⇒`std` a metà
implementazione.

## File toccati

- docs/superpowers/plans/2026-06-12-wasm-mt-fase2-threads.md (nuovo)
