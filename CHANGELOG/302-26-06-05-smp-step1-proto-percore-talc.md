# 302 — SMP Step 1: Prototype B — per-core talc + remote-free queue

**Data:** 2026-06-05

## Cosa
Aggiunto Prototype B (`alloc_percore_talc`) per la spike SMP Step 1 allocator.
- Nuovo file `kernel/src/memory/alloc_percore_talc.rs`: `PerCoreTalc` con `MAX_CPUS`
  arene `Talck` su sub-span disgiunti (metà heap), un fallback `Talck` globale per
  allocazioni >=64 KiB (seconda metà heap), e una remote-free queue `IrqMutex<VecDeque>`
  per-core per liberazioni cross-core. Selezionato con cargo feature `alloc-percore-talc`.
- `kernel/src/memory/heap.rs`: aggiunto il terzo ramo `#[global_allocator]` per
  `alloc-percore-talc` e corretti i due rami `cfg` di `init_heap` da
  `not(alloc-magazine)` / `alloc-magazine` a `not(any(alloc-magazine, alloc-percore-talc))`
  / `any(alloc-magazine, alloc-percore-talc)` (entrambi espongono `claim(base, size)`).
- `kernel/src/memory/mod.rs`: aggiunto `pub mod alloc_percore_talc` sotto
  `#[cfg(feature = "alloc-percore-talc")]`.

Risultati benchmark (boot-checks + alloc-percore-talc, QEMU q35 1 core):
```
allocbench cpuid    ns_per_call=221 iters=100000 acc=0
allocbench single   small_ns=164  large_ns=493   iters=100000 acc=0xFBC520
allocbench multi    cores=1 total_ns=10514032 per_job=10514032 jobs=1 sink=0x4A811AD8
```
Entrambe le build (default e alloc-percore-talc) passano `TEST_BOOT_PASS`.

## Perché
Spike di misura per confronto con Prototype A (magazine) nel Task 6, nell'ambito
della migrazione SMP shared-nothing (spec 2026-06-05). THROWAWAY code — non di produzione.

## File toccati
- `kernel/src/memory/alloc_percore_talc.rs` (nuovo)
- `kernel/src/memory/heap.rs`
- `kernel/src/memory/mod.rs`
- `CHANGELOG/302-26-06-05-smp-step1-proto-percore-talc.md` (questo file)
