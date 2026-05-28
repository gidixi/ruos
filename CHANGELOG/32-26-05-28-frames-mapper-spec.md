# 32 — Spec design: Frame allocator + Mapper API (Step 6)

**Data:** 2026-05-28

## Cosa

Scritta la spec dello Step 6 in
`docs/superpowers/specs/2026-05-28-rust-frames-mapper-design.md`. Architettura:
bitmap frame allocator (heap-backed, no cap, costruito da Limine memory map) +
Mapper globale (`x86_64::OffsetPageTable` con HHDM offset) + helper `map_page`/
`unmap_page`/`map_io_page`. Refactor di `apic/mmio.rs` per usare il nuovo
Mapper (cancellazione del page-walk manuale + `Box::leak` + guardia
`HUGE_PAGE` ad-hoc — quest'ultima ora è gestita dal crate `x86_64`).

Decomposizione 3 task:
1. Split `memory.rs` in `memory/mod.rs` + `heap.rs` + `frames.rs`.
2. `memory/mapper.rs` + smoke test map/unmap a virt `0x4000000000`.
3. Refactor `apic/mmio.rs` → `memory::map_io_page`.

## Perché

Step 6 della roadmap WASM-first: paging API unificata necessaria per Step 7+
(VFS, framebuffer DMA, virtio rings, WASM linear memory growth). Niente
per-process page tables, niente ring 3 — kernel-system paging only.

## File toccati

- docs/superpowers/specs/2026-05-28-rust-frames-mapper-design.md
- CHANGELOG/32-26-05-28-frames-mapper-spec.md
