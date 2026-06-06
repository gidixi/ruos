# 313 — SMP: RNG per-core + supervisor 6-detect

**Data:** 2026-06-06

## Cosa
Due item bundled:

**PART 1 — RNG per-core (`kernel/src/rng.rs`):**
- Sostituisce `static RNG: Mutex<Option<ChaCha20Rng>>` singolo con `[RngSlot; MAX_CPUS]`.
- BSP semina TUTTI i MAX_CPUS slot a `init()` con RDRAND draw distinti → stream ChaCha20 separati per core.
- `fill`/`next_u64` indicizzano `RNG[cpu_id()]` → zero lock cross-core.
- Boot-check (boot-checks): invia un draw su un ComputeApp AP via message bus e verifica `bsp != ap` → `true`.

**PART 2 — Supervisor 6-detect (`kernel/src/sched/cpustat.rs`, `kernel/src/executor/mod.rs`, `kernel/src/wasm/wt/wm.rs`):**
- Aggiunge `HEARTBEAT: [AtomicU64; MAX_CPUS]` in `cpustat.rs` con `heartbeat_bump(cpu)` / `heartbeat(cpu)`.
- Bump in OGNI main loop di ogni core:
  - `executor::run_core`: bump all'inizio di ogni iterazione del loop (BSP + ComputeApp AP).
  - `wm::Compositor::run`: bump ogni frame del compositor (GUI core post-hand-off).
  - `wm::gui_worker_loop`: bump ogni wake dal hlt mentre aspetta il hand-off (GUI core pre-compositor).
- `supervisor_task` async (BSP, embassy): snapshot ogni ~1s, confronta, logga `supervisor up, watching N cores` / `all N cores alive` / `mute cores=...`.
- Correzioni boot-check preesistenti (Step 2 inbox roundtrip, Step 3b heartbeat, Step 3c cross-spawn): ora targettano un ComputeApp AP (core 2+) invece di core 1 (GuiCompositor, che non drena inbox né ha executor).

## Perché
- RNG per-core elimina la lock cross-core su `random_get` / SSH keygen (zero contention).
- Supervisor 6-detect dà visibilità liveness: ogni secondo sa se tutti i core avanzano; mute-core rilevato prima della recovery (6-recover, step successivo).
- Fix boot-check necessario: Step 5 ha spostato core 1 in `gui_worker_loop`; le check esistenti inviavano messaggi a core 1 che non drena inbox → timeout/panic.

## File toccati
- `kernel/src/rng.rs`
- `kernel/src/sched/cpustat.rs`
- `kernel/src/executor/mod.rs`
- `kernel/src/wasm/wt/wm.rs`
- `kernel/src/boot/phases/interrupts.rs`
- `CHANGELOG/313-26-06-06-smp-rng-percore-supervisor-6detect.md`
