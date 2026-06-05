# 299 — SMP Step 1: multi-core allocator contention benchmark

**Data:** 2026-06-05

## Cosa

Aggiunto `run_multicore()` in `kernel/src/memory/allocbench.rs`: sottomette un job
`alloc_churn_job` per core online tramite `smp::pool`, attende i risultati con il
BSP come worker di fallback, misura il wall-time TSC totale e conta i core distinti
che hanno eseguito i job. Stampa il marker greppabile:
`allocbench multi cores=<N> total_ns=<N> per_job=<N> jobs=<N> sink=0x<..>`.

La chiamata è aggiunta in `boot/phases/interrupts.rs` immediatamente dopo
`smp::bringup()`, gated su `#[cfg(feature = "boot-checks")]`.

## Perché

Misurare la contesa cross-core sull'allocatore in modo identico tra i prototipi
(talc globale / magazine / per-core talc) è il requisito del SMP shared-nothing
Step 1. Task 3 della spike.

## File toccati

- kernel/src/memory/allocbench.rs
- kernel/src/boot/phases/interrupts.rs
