# 118 — pci/mod.rs: init/enumerate/find_class/API

**Data:** 2026-05-29

## Cosa
`pci::init` (scan piatto bus/device/function + multifunction probe), global
`Once<PciState>`, `devices()` (clone), `find_class`, `PciDevice::{bar,
enable_mmio, enable_bus_master}`, `PciError`, `PciInitInfo { device_count, xhci }`.

## Perché
API ruos di discovery sopra pci_types per i consumer (xHCI/AHCI/net).

## File toccati
- kernel/src/pci/mod.rs
