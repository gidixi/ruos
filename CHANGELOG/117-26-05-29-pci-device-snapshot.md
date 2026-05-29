# 117 — pci/device.rs: PciDevice da pci_types

**Data:** 2026-05-29

## Cosa
`PciDevice::probe` costruisce uno snapshot (ids, class/subclass/prog_if, 6 BAR
decodificati+sized) via pci_types `PciHeader`/`EndpointHeader`. Re-export `Bar`.

## Perché
Cache owned per i consumer; nessuna decodifica BAR/header a mano.

## File toccati
- kernel/src/pci/device.rs
