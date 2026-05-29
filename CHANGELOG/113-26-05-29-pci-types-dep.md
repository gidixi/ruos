# 113 — Aggiunta dipendenza pci_types

**Data:** 2026-05-29

## Cosa
Aggiunto `pci_types = "0.10"` a `kernel/Cargo.toml` (no_std, MIT/Apache).

## Perché
Step 13 PCI/ECAM delega a `pci_types` la decodifica del config-space (header,
BAR+sizing, 64-bit, capability walk, command/status), invece di riscriverla.

## File toccati
- kernel/Cargo.toml
