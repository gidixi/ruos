# 105 — Piano implementazione Step 13 PCI/ECAM

**Data:** 2026-05-29

## Cosa
Scritto il piano d'implementazione `docs/superpowers/plans/2026-05-29-pci-ecam.md`
(skill superpowers:writing-plans): 9 task bite-sized TDD-integrazione (branch,
dep pci_types, q35+xhci, estrazione MCFG, EcamAccess, PciDevice, enum/find_class,
fase boot+smoke, hardening run-test, docs). Path/firme allineati al codice reale
(boot/phases, binfo!, map_io_page, AcpiInfo). Test = boot QEMU + grep seriale.

## Perché
Tradurre la spec Step 13 (rev. pci_types ibrida) in passi eseguibili per un
engineer senza contesto del codebase.

## File toccati
- docs/superpowers/plans/2026-05-29-pci-ecam.md
