# 115 — acpi_init: estrazione ECAM da MCFG

**Data:** 2026-05-29

## Cosa
`EcamRegion` + `AcpiInfo.ecam` (Vec) popolato in `parse()` via
`acpi::mcfg::PciConfigRegions`; variante `AcpiInitError::NoMcfg` (non fatale).
Log mem phase mostra `ecam=N`.

## Perché
Fornire le finestre ECAM al modulo PCI dello Step 13 riusando l'ACPI già parsato.

## File toccati
- kernel/src/acpi_init.rs
- kernel/src/boot/phases/mem.rs
