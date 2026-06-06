# 308 — SMP Step 3a: per-core Delay lists + AP LAPIC timer

**Data:** 2026-06-06

## Cosa

Implementazione completa dello Sub-step 3a del piano SMP Step 3 (per-core executor).

1. **Per-core Delay lists** (`executor/delay.rs`): rimossa la singola lista globale
   `SLOTS_LIST`; introdotta `PER_CORE_DELAYS: [DelayList; MAX_CPUS]`. `my_list()`
   risolve la lista del core corrente via `cpu_id()`. `free_slot` e il path
   task-side di `poll` usano `my_list()`. La nuova `timer_tick_core(now, cpu)`
   drena `PER_CORE_DELAYS[cpu]` con `try_lock` (ISR lock-light, invariante spec
   inv. 5 + 6). `GEN_COUNTER` rimane globale (atomico, fine).

2. **Timer handler per-core** (`timer.rs`): `timer_handler` ora legge `cpu_id()`;
   solo il BSP (cpu==0) chiama `TICKS.fetch_add` + `tick_cursor()` (invariante
   single-writer, spec inv. 8); gli AP leggono TICKS e incrementano il proprio
   `AP_TICKS[cpu]`. Tutti i core chiamano `timer_tick_core(now, cpu)`.
   Guard bounds-check su `cpu >= MAX_CPUS` per il periodo di probe della
   `probe_fast_cpuid` (TSC_AUX = 0xABCD transitorio). Aggiunto `AP_TIMER_COUNT`
   (AtomicU32) pubblicato da `init()` prima di armare il timer BSP, e
   `start_ap_timer()` per gli AP. Aggiunto `ap_ticks(cpu)` per il boot-check.

3. **AP timer armato** (`cpu/ap.rs`): dopo `lapic::init_ap` (che maschera il
   LVT timer), `ap_entry` chiama `timer::start_ap_timer()` che reprogramma il
   LVT con `set_timer_periodic(VEC_LAPIC_TIMER, count)` — unmasked, periodico.
   Gli AP ricevono ora IRQ timer a 100 Hz e drenano `PER_CORE_DELAYS[cpu_id()]`.

4. **Boot-check gate** (`boot/phases/interrupts.rs`): con feature `boot-checks`
   e `cpus_online >= 2`, il BSP attende ~5 tick (50 ms) e confronta
   `ap_ticks(1)` prima e dopo — deve crescere (N > 0). Con 1 core: skipped.

## Perché

Prerequisito per i per-core executor (Step 3b): ogni AP deve poter drenare la
propria Delay list sul proprio tick. Prima di questo passo tutti i Delay futuri
venivano gestiti dal BSP su un'unica lista globale; ora ogni core ha la propria
(single-writer-per-slot, nessun cross-core writer). Il gate conferma che il
timer LAPIC dell'AP effettivamente scatta.

## Risultati gate

- `make test-boot` (1 core): `TEST_BOOT_PASS`
- `make iso CARGO_FEATURES="boot-checks"` + QEMU -smp 4:
  `ap1 ticks in 50ms = 78 (expect > 0)` ✓
- `make run-smp-test`: `TEST_PASS_SMP`
- `make run-smp2-test`: `TEST_PASS_SMP2`

## File toccati

- `kernel/src/executor/delay.rs`
- `kernel/src/timer.rs`
- `kernel/src/cpu/ap.rs`
- `kernel/src/boot/phases/interrupts.rs`
- `CHANGELOG/308-26-06-06-smp-step3a-percore-delay-ap-timer.md`
