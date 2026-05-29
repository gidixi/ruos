# 101 — Roadmap: Step 13 DONE, inserito Step 14 PCI/ECAM, rinumerati gli step

**Data:** 2026-05-29

## Cosa
- Step 13 (Mouse PS/2 + rlvgl) marcato **✅ DONE**.
- Inserito **Step 14 — PCI/PCIe enumeration (ECAM)** subito dopo lo Step 13
  (estrazione MCFG, modulo `pci/`, `find_class`/BAR decode/Command bits/cap walk;
  spec `2026-05-29-rust-pci-ecam-design.md`).
- Rinumerati gli step seguenti: Networking 14→15, SSH 15→16, SMP 16→17,
  AHCI 17→18 (aggiunto nell'entry 100).
- Aggiornati tutti i cross-reference interni (CSPRNG→Step 15, SSH→Step 16,
  post-Step-16, Step 16.5+, dipendenze AHCI→Step 14) e il diagramma di dipendenza.

## Perché
Step 13 completato. PCI/ECAM aveva spec ma non era uno step numerato: è la
fondamenta comune a virtio-net (Step 15), AHCI (Step 18) e futuri NVMe/xHCI, e va
prima di tutti loro. Collocato dopo lo Step 13 appena finito.

## File toccati
- docs/superpowers/roadmap-rust-os.md
