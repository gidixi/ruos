# 487 — Gate 1 MT Fase 2: SharedMemory + atomics nativi nell'engine no_std

**Data:** 2026-06-12

## Cosa

Primo bring-up gate della Fase 2 multithreading WASM (marker `THREADS-OK 1`):

- nuovo guest `tools/wt-threads-gate/gate1.wat` — modulo CORE che importa una
  memoria `shared` (`(memory 1 1 shared)`), esegue due `i32.atomic.rmw.add`
  (compilati AOT a istruzioni lock-prefixed x86) e rilegge il valore con
  `i32.atomic.load`: `run()` deve restituire 42;
- regola Makefile `$(WT_KDIR)/threads_gate1.cwasm` (precompile via
  wt-precompile, che già ha `wasm_threads(true)` dal task 0) + aggiunta a
  `WT_KCWASMS`;
- runner kernel `run_threads_gate1()` in `kernel/src/wasm/wt/mod.rs`
  (gated `boot-checks`): crea la `wasmtime::SharedMemory` host-side dal
  `MemoryType` importato dal guest, la definisce nel linker come `env.memory`,
  istanzia ed esegue `run` (con `cld` prima della call, come ogni call site wt);
- chiamata nel blocco boot-checks di `boot/phases/interrupts.rs` accanto a
  `run_bringup_demo`, log `THREADS-OK 1 = ok|FAIL`;
- `config.shared_memory(true)` nell'`engine_config()` del kernel: il fork ha
  un gate runtime-only (default false, controllato in
  `vm/memory/shared_memory.rs::wrap`) che senza flag fa fallire
  `SharedMemory::new` con "shared memory support is disabled for this
  engine". Il knob NON è hashato nel .cwasm (non è nei Tunables) → nessun
  impatto sulla compatibilità dei .cwasm esistenti, wt-precompile invariato.

Il gate NON usa `atomic.wait/notify`: prova solo SharedMemory + atomics
nativi nel fork `third_party/wasmtime45` (feature `threads` in no_std); gli
stub futex temporanei di platform.rs restano fuori dal percorso.

Verificato in QEMU (q35, -smp 4, -m 2048, boot-checks): `THREADS-OK 1 = ok`.

## Perché

La Fase 2 MT poggia su SharedMemory + atomics del fork wasmtime no_std
(task 0, changelog 486): serve un gate end-to-end che dimostri che l'engine
runtime-only deserializza un .cwasm con feature THREADS, alloca una shared
memory host-side e esegue RMW atomici reali — prima di costruire sopra
spawn/join e wait/notify. Se il fork regredisce, questo marker lo dice subito.

## File toccati

- tools/wt-threads-gate/gate1.wat (nuovo)
- Makefile
- kernel/src/wasm/wt/mod.rs
- kernel/src/boot/phases/interrupts.rs
