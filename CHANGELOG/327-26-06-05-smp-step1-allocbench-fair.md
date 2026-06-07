# 301 — allocbench: measure all benches post-LAPIC + cpu_id micro-bench

**Data:** 2026-06-05

## Cosa

- Rimosso `run_single_core()` dal blocco `boot-checks` in `boot/phases/mem.rs`
  (fase mem, pre-LAPIC).
- In `boot/phases/interrupts.rs`, il singolo `run_multicore()` è sostituito da un
  blocco ordinato che esegue: `run_cpuid_bench()` → `run_single_core()` →
  `run_multicore()`, tutti post-LAPIC (dopo `smp::bringup()`).
- Aggiunta la funzione `run_cpuid_bench()` in `kernel/src/memory/allocbench.rs`:
  misura il costo di 100 000 chiamate a `cpu_id()` (LAPIC MMIO read) e stampa
  `allocbench cpuid ns_per_call=… iters=… acc=…`.

## Perché

Una code review ha rilevato che il bench single-core girava prima dell'init LAPIC
(cpu_id() ritornava 0 a ~5 ns) mentre il bench multi-core girava dopo (LAPIC MMIO
read reale, ~100 ns). I prototipi per-core chiamano `cpu_id()` su ogni alloc: le
due misure avevano costi cpu_id diversi → confronto falsato. Ora entrambi i bench
girano con lo stesso costo cpu_id, rendendo il confronto equo per la decisione
Task 6.

## File toccati

- `kernel/src/memory/allocbench.rs`
- `kernel/src/boot/phases/mem.rs`
- `kernel/src/boot/phases/interrupts.rs`
- `CHANGELOG/301-26-06-05-smp-step1-allocbench-fair.md`
