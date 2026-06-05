# 300 — SMP Step 1 Prototype A: per-core magazine allocator (alloc-magazine)

**Data:** 2026-06-05

## Cosa
Implementato il Prototipo A del piano SMP shared-nothing Step 1: un magazine cache
per-core davanti all'allocatore talc globale, selezionabile con il feature
`alloc-magazine`. Modifiche:

- `kernel/Cargo.toml`: aggiunti feature `alloc-magazine = []` e
  `alloc-percore-talc = []` (quest'ultimo usato dal Prototipo B, Task 5).
- `kernel/src/memory/alloc_magazine.rs`: nuovo file con `MagazineAlloc`
  (`#[global_allocator]` sotto `alloc-magazine`). Ogni classe alloca/dealloca da
  talc con la Layout canonica della classe (size=16<<idx, align=16), così tutti i
  blocchi in magazine hanno dimensione nota e non si verifica overflow su recycle.
  Bypass per `align > 16` (refinement di correttezza: talc allinea correttamente).
  Free-list intrusiva con profondità CACHE_DEPTH=64 per (core, classe).
- `kernel/src/memory/heap.rs`: `#[global_allocator]` cfg-gated:
  `not(any(alloc-magazine, alloc-percore-talc))` → talc baseline;
  `alloc-magazine` → `MagazineAlloc`. `init_heap` cfg-gate sulla `claim`.
- `kernel/src/memory/mod.rs`: `pub mod alloc_magazine` sotto
  `#[cfg(feature = "alloc-magazine")]`.
- `kernel/src/apic/lapic.rs`: `apic_id()` resa sicura in pre-LAPIC (ritorna 0
  se `LAPIC_VIRT == 0`), evitando #PF da accesso MMIO non ancora mappato durante
  le alloc early-boot del magazine.

## Perché
Spike di misura: confrontare la latenza alloc/free del magazine per-core vs il
Prototipo B (per-core talc, Task 5) e vs il baseline globale (Task 2/3). Il
feature `alloc-magazine` è throwaway — selezionato solo per il benchmark.

Risultati benchmark (QEMU, 1 CPU, boot-checks + alloc-magazine):
- `allocbench single small_ns=110 large_ns=419` (magazine)
- Baseline (talc globale): `small_ns=185 large_ns=465`
- Speedup small: ~40% (hit di cache, nessun lock CAS cross-core)
- Large (>MAX_SMALL=2048): bypass magazine → latenza simile al baseline

## File toccati
- `kernel/Cargo.toml`
- `kernel/src/memory/alloc_magazine.rs` (nuovo)
- `kernel/src/memory/heap.rs`
- `kernel/src/memory/mod.rs`
- `kernel/src/apic/lapic.rs`
- `CHANGELOG/300-26-06-05-smp-step1-proto-magazine.md` (questo file)
