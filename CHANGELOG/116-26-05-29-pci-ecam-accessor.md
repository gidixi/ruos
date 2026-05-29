# 116 — pci/ecam.rs: EcamAccess (ConfigRegionAccess)

**Data:** 2026-05-29

## Cosa
`kernel/src/pci/ecam.rs`: `EcamAccess` implementa `pci_types::ConfigRegionAccess`
(calcolo indirizzo fisico ECAM + read/write volatile u32 su `map_io_page`).
Dichiarato `mod pci;` in main.rs; stub `pci/mod.rs` + `pci/device.rs`.

## Perché
Unico codice config-space kernel-specifico; pci_types decodifica sopra.

## File toccati
- kernel/src/pci/ecam.rs
- kernel/src/pci/mod.rs
- kernel/src/pci/device.rs
- kernel/src/main.rs
