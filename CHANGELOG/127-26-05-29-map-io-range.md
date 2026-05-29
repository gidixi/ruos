# 127 — mapper::map_io_range (BAR multi-pagina)

**Data:** 2026-05-29

## Cosa
`map_io_range(phys, bytes)`: mappa tutte le pagine MMIO del range (uncached),
ritorna il virt di phys. Per le finestre BAR virtio.

## File toccati
- kernel/src/memory/mapper.rs
- kernel/src/memory/mod.rs
