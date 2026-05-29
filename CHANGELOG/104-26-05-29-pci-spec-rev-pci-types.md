# 104 — Spec Step 13: revisione PCI/ECAM → riuso crate pci_types (ibrido)

**Data:** 2026-05-29

## Cosa
Revisionata la spec `docs/superpowers/specs/2026-05-29-rust-pci-ecam-design.md`
per non riscrivere da zero la decodifica del config-space PCI.

- Nuova sezione **"Prior art & reuse decision"**: analisi di `toku-sa-n/ramen`
  (NON riusabile: GPL-3.0, porte legacy `0xCF8/0xCFC`, microkernel via syscall,
  minimale senza sizing/cap walk). Annotati per lo step xHCI futuro i crate
  `xhci` 0.9.2 + `accessor` (stesso autore, MIT, no_std).
- **Decisione (ibrida):** delegare header parse / BAR decode+sizing / 64-bit /
  capability walk / Command-Status registers al crate **`pci_types`** (rust-osdev,
  MIT/Apache, no_std, si sposa col crate `acpi`). Unico codice kernel-specifico
  che restiamo a scrivere = `EcamAccess` che implementa
  `pci_types::ConfigRegionAccess` (2 metodi `unsafe` su `map_io_page` + volatile).
  Sopra teniamo l'API ruos (`PciDevice`/`find_class`/`bar`/`enable_*`).
- Riscritti Goals, diagramma Architecture, Component 2 (`pci/ecam.rs` =
  `EcamAccess`), Component 3 (`pci/device.rs` = snapshot da `pci_types`),
  Component 4 (API + global `(EcamAccess, Vec<PciDevice>)`), kmain block, data
  flow, error handling, decomposition (aggiunto task 0 = dep `pci_types`),
  Open items (alcune decise: snapshot clone, ownership mapping BAR via
  `map_io_range`), header/status (ora Step 13).

## Perché
Richiesta utente: rivedere la spec controllando come ramen ha gestito il PCI per
evitare reimplementazione. Esito: ramen non riusabile direttamente, ma esiste un
crate mantenuto (`pci_types`) che copre tutta la decodifica. Riduce codice e
superficie di bug; resta nostro solo l'accesso ECAM (kernel-specifico).

## File toccati
- docs/superpowers/specs/2026-05-29-rust-pci-ecam-design.md
