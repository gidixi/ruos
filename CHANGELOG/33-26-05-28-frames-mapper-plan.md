# 33 — Piano implementazione: Frame allocator + Mapper (Step 6)

**Data:** 2026-05-28

## Cosa

Scritto il piano dello Step 6 in
`docs/superpowers/plans/2026-05-28-rust-frames-mapper.md`. Tre task:

1. **Frame allocator** — split `memory.rs` in `memory/{mod,heap,frames}.rs`;
   bitmap su heap inizializzata da Limine memmap (max_phys, USABLE → free,
   heap region → re-used); impl `FrameAllocator<Size4KiB>` +
   `FrameDeallocator<Size4KiB>` per integrazione con `x86_64::Mapper`.
2. **Mapper wrapper + smoke test** — `memory/mapper.rs` con
   `OffsetPageTable` globale; helper `map_page`/`unmap_page`/`map_io_page` +
   errori tipizzati `MapError`/`UnmapError`; smoke test al boot mappa
   `0x4000000000`, scrive/legge u64, unmap, free. Log `paging up` +
   `map test ok`.
3. **Refactor `apic/mmio.rs`** → `memory::map_io_page`; cancella file MMIO
   custom; `lapic::init` e `ioapic::init` perdono parametro `hhdm_offset`.

`TEST_PASS` preservato a ogni checkpoint (Makefile assert resta
`ruos: ticks=`).

## Perché

Tradurre lo spec Step 6 in passi eseguibili e verificabili.

## File toccati

- docs/superpowers/plans/2026-05-28-rust-frames-mapper.md
- CHANGELOG/33-26-05-28-frames-mapper-plan.md
