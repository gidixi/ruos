# 36 — Refactor `apic/mmio.rs` → `memory::map_io_page`

**Data:** 2026-05-28

## Cosa
- `apic/lapic.rs::init` e `apic/ioapic.rs::init` perdono il parametro
  `hhdm_offset`; entrambi chiamano `crate::memory::map_io_page(phys)` per
  ottenere il virt UC e procedono col loro setup MMIO.
- Cancellato `kernel/src/apic/mmio.rs` (page-walk manuale + `Box::leak` PT
  pages + guardia `HUGE_PAGE` ad-hoc + counter `LEAKED`). Semantica
  equivalente ora vive nel typed `OffsetPageTable` di Task 2.
- `kmain` aggiornato: passa solo `phys_base` + `spurious_vector` a
  `lapic::init`, solo `phys_base` a `ioapic::init`.

## Perché
Terzo pezzo dello Step 6: una sola API paging in tutto il kernel.

## File toccati
- kernel/src/apic/lapic.rs
- kernel/src/apic/ioapic.rs
- kernel/src/apic/mod.rs
- kernel/src/apic/mmio.rs (rimosso)
- kernel/src/main.rs
- CHANGELOG/36-26-05-28-mmio-refactor.md
