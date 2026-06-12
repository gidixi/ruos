# 488 — MT Fase 2 Task 2: fiber runtime per i wasm-thread (threads.rs) + self-test

**Data:** 2026-06-12

## Cosa

Nucleo dello scheduler M:N dei wasm-thread (`kernel/src/wasm/wt/threads.rs`),
basato su `wasmtime-internal-fiber` 45 (backend no_std: stack su heap, switch
asm, zero statics → SMP-safe con ownership esclusiva del fiber):

- **`ThreadFiber`** — fiber + TLS wasmtime salvato + suspend handle + park
  state (`park_key`/`park_deadline`); `Send` via ownership esclusiva
  (RUNQ/WAITQ ↔ un solo core in `run_one`).
- **`ThreadGroup`** — stato condiviso di un'app threaded (Module +
  SharedMemory + Linker + atomics live/poisoned/exit); usato da
  `exec_threaded` dal Task 3.
- **`RUNQ`** globale + **`WAITQ`** futex sharded ×16 (`align(64)`), con
  protocollo anti-lost-wakeup a *crediti*: il park della Box avviene in
  `run_one` (che la possiede) DOPO il suspend; un `wake_key` che incrocia un
  parker "in volo" lascia un credito che `run_one` consuma prima di inserire
  in WAITQ.
- **`run_one(cpu)`** — dequeue + **TLS swap** (l'activation chain wasmtime di
  una call sospesa vive nello stack del fiber: il puntatore TLS per-core
  viaggia con lui, permettendo anche la migrazione cross-core) + resume +
  dispatch Ok(finito)/Err(parcheggiato).
- **Seam in `run_core()`** (`executor/mod.rs`): drain dei fiber dopo il pool
  (`core_allowed` = ComputeApp, fallback BSP su 1-2 core) + run-queue fiber
  nella disgiunzione wake-source.
- **`tls_raw_get/set`** in `wt/platform.rs` (load/store del TLS per-core, non
  `extern "C"`).
- **Self-test boot-checks** `fiber_self_test()`: fiber host-only che pubblica
  il Suspend, si parcheggia su una chiave di test, viene svegliato dal BSP con
  `wake_key` e finisce — marker `THREADS-FIBER-OK = ok ran_on=N resumed_on=M`
  (con ≥3 core N,M = core ComputeApp; su 1-2 core il BSP drena da sé).

Dep nuova: `wasmtime-internal-fiber = "=45.0.0"` (default-features = false).

## Perché

MT Fase 2 Task 2 (piano
`docs/superpowers/plans/2026-06-12-wasm-mt-fase2-threads.md`): i thread wasm
(e il main dei moduli threaded) devono girare su fiber con stack-switch reale,
così l'hook futex (Task 4) può sospendere un thread che fa `atomic.wait`
cedendo il core invece di bloccarlo. Questo task posa scheduler + TLS swap +
protocollo park/wake e li prova host-only, senza ancora toccare il path wasm.

## File toccati

- kernel/src/wasm/wt/threads.rs (nuovo)
- kernel/src/wasm/wt/platform.rs
- kernel/src/wasm/wt/mod.rs
- kernel/src/executor/mod.rs
- kernel/src/boot/phases/interrupts.rs
- kernel/Cargo.toml
- kernel/Cargo.lock
- CHANGELOG/488-26-06-12-wt-threads-fiber-runtime.md
