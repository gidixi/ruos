# 126 — memory/dma + frame allocazione contigua

**Data:** 2026-05-29

## Cosa
`frames::allocate_contiguous/free_contiguous` (scan bitmap per N frame
consecutivi). `memory/dma.rs`: `DmaRegion` + alloc/dealloc (HHDM alias,
zero-init). `mapper::hhdm_virt`.

## Perché
Le ring/buffer virtio (e AHCI) richiedono memoria DMA fisicamente contigua.

## File toccati
- kernel/src/memory/frames.rs
- kernel/src/memory/dma.rs
- kernel/src/memory/mapper.rs
- kernel/src/memory/mod.rs
