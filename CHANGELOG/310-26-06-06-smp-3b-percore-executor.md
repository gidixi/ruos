# 310 — Step 3b: per-core executor, APs run run_core, AP1 heartbeat boot-check

**Data:** 2026-06-06

## Cosa

- `kernel/src/executor/mod.rs`: sostituito il singleton `EXECUTOR` con
  `PER_CORE_EXECUTOR[MAX_CPUS]`. Aggiunta `run_core(cpu)`: inizializza il slot
  di questo core con context=cpu_id, spawna i task I/O su cpu==0 (BSP, behaviour
  invariato), spawna `heartbeat_task` su cpu==1 sotto `boot-checks`, poi loop:
  poll → drain_inbox → drain compute pool → hlt gated su
  WAKE_PENDING||inbox||pool. `run()` diventa thin wrapper su `run_core(0)`.
  Aggiunto `HEARTBEAT: AtomicU64` (boot-checks) per il gate AP1.
- `kernel/src/cpu/ap.rs`: `ap_entry` chiama `executor::run_core(cpu_id as u32)`
  al posto di `ap_worker_loop()`. `ap_worker_loop` eliminata — la sua logica
  (pool drain + inbox drain + hlt) vive ora in `run_core`, condivisa da tutti i
  core. Aggiornato il doc del modulo.
- `kernel/src/boot/phases/interrupts.rs`: aggiunto boot-check Step 3b — attende
  ~100 ms (10 tick BSP) e verifica che `HEARTBEAT` sia cresciuto > 0 (atteso ~5,
  uno ogni Delay::ticks(2) ≈ 20 ms). Prova la catena completa: executor AP1 →
  Delay per-core → LAPIC timer AP1.

## Perché

Foundation per Step 3c (cross-core task spawn via queue+IPI) e Step 5 (task
pinning per core). Ogni core ora gestisce il proprio executor cooperativo
(poll-queue + Delay list + pool drain) in modo autonomo. Il pool di calcolo
(banded compositing) continua a funzionare perché il drain è integrato nel loop
di `run_core` su ogni core.

**Gate (verificato):**
- `make test-boot` (1 core) → `TEST_BOOT_PASS` (BSP path invariato)
- boot-checks -smp 4, 2 run: `ap1 heartbeat ticks in 100ms = 5` (entrambe le run)
- `make run-smp-test` → `TEST_PASS_SMP`
- `make run-smp2-test` → `TEST_PASS_SMP2` (speedup 2.78x, 3 core distinti)

## File toccati

- `kernel/src/executor/mod.rs`
- `kernel/src/cpu/ap.rs`
- `kernel/src/boot/phases/interrupts.rs`
- `CHANGELOG/310-26-06-06-smp-3b-percore-executor.md`
