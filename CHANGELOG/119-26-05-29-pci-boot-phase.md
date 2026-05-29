# 119 — boot: fase PCI + smoke log

**Data:** 2026-05-29

## Cosa
Nuova fase `boot/phases/pci.rs` (dopo `interrupts`): chiama `pci::init`, logga
`init ok devices=N`, `xhci @ bb:dd.f`, `xhci bar0=... size=...`. `BootError::PciInit`.
Non fatale se ECAM assente.

## Perché
Wiring Step 13 + smoke verificabile su seriale.

## File toccati
- kernel/src/boot/phases/pci.rs
- kernel/src/boot/phases/mod.rs
- kernel/src/boot/mod.rs
- kernel/src/boot/error.rs
