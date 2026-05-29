# 121 — Roadmap: Step 13 PCI/ECAM completato

**Data:** 2026-05-29

## Cosa
Step 13 (PCI/ECAM) marcato ✅ DONE.

## Perché
Implementazione completa e verificata: `make run-test` asserisce
`pci init ok devices>=1` + `xhci @ ...` (q35 enumera 7 device, xHCI @ 00:03.0,
BAR0 64-bit 0xFEBD4000 size 0x4000 decodificato e sized).

## File toccati
- docs/superpowers/roadmap-rust-os.md
