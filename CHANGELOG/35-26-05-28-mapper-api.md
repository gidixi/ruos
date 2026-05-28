# 35 — Mapper API (`OffsetPageTable`) + smoke test boot

**Data:** 2026-05-28

## Cosa
- `kernel/src/memory/mapper.rs`: wrapper di `x86_64::OffsetPageTable` con
  HHDM offset; helper `init(hhdm)`, `map_page`, `unmap_page`, `map_io_page`.
- Errori tipizzati `MapError`/`UnmapError` con `Display` (proiettano
  `MapToError`/`UnmapError` del crate `x86_64`).
- `memory/mod.rs` re-export.
- `kmain`: chiama `init_mapper`, logga `ruos: paging up`, esegue smoke test
  map/unmap su `0x4000000000` (PML4[128] fresco), logga `ruos: map test ok`.
- `apic/mmio.rs` ancora attivo per LAPIC/IOAPIC; Task 3 lo rifattora.

## Perché
Secondo pezzo dello Step 6: API paging unificata su trait `x86_64::Mapper`
con frame allocator come consumer.

## File toccati
- kernel/src/memory/mapper.rs (nuovo)
- kernel/src/memory/mod.rs
- kernel/src/main.rs
- CHANGELOG/35-26-05-28-mapper-api.md
