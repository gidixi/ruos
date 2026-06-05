# 298 — Single-core allocator micro-benchmark (boot-checks)

**Data:** 2026-06-05

## Cosa
Aggiunto `kernel/src/memory/allocbench.rs`: micro-benchmark di latenza alloc+free
compilato solo sotto la feature `boot-checks`. Misura cicli TSC per 100 000
allocazioni piccole (Box<u64>, ~64 B) e 256 allocazioni grandi (Vec<u8> da 1 MiB),
converte in nanosecondi via `tsc_per_ms()` e stampa il marker greppabile:
`allocbench single small_ns=<N> large_ns=<N> iters=<N> acc=0x<HEX>`.

Wiring:
- `kernel/src/memory/mod.rs`: `pub mod allocbench` sotto `#[cfg(feature = "boot-checks")]`.
- `kernel/src/boot/phases/mem.rs`: chiamata `crate::memory::allocbench::run_single_core()`
  appesa in fondo al blocco `boot-checks` esistente (dopo lo smoke test heap).

## Perché
Task 2 dello spike "SMP shared-nothing Step 1": costruire lo strumento di misura
prima dei prototipi di allocatore (talc magazine / per-core talc). I numeri
prodotti qui sono il baseline default-talc che Tasks 4/5 confronteranno.

## File toccati
- kernel/src/memory/allocbench.rs  (nuovo)
- kernel/src/memory/mod.rs
- kernel/src/boot/phases/mem.rs
- CHANGELOG/298-26-06-05-smp-step1-allocbench.md  (questo file)
