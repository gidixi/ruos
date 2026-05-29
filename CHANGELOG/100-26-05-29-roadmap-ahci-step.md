# 100 — Roadmap: aggiunto Step 17 AHCI/SATA + FAT persistente

**Data:** 2026-05-29

## Cosa
Aggiunto **Step 17 — AHCI / SATA disk + FAT persistente** alla roadmap:
discovery via PCI (`find_class 0x01/06/01` → ABAR/BAR5), HBA + port bring-up
(command list / FIS / PRDT in buffer DMA contigui), ATA `IDENTIFY` + `READ/WRITE
DMA EXT` (LBA48), nuovo trait `BlockDevice`, FAT (`fatfs` no_std) montato nel VFS,
helper allocator DMA sopra il frame allocator. Prerequisito esplicito: lo step
PCI/ECAM (spec `2026-05-29-rust-pci-ecam-design.md`).

Aggiornati anche il diagramma di dipendenza (catena Step 6/7 → PCI ECAM → AHCI →
NVMe/xHCI/virtio-net) e la sezione "Cosa NON è in roadmap" (FAT/AHCI non più
genericamente "dopo" ma spostati allo Step 17).

## Perché
Discussione su come arrivare al controllo di un disco fisico: `crab-usb` è solo
trasporto USB (servirebbe mass-storage + SCSI + block layer sopra), mentre AHCI
(SATA) è la rotta corta allo storage persistente. Entrambe partono comunque dal
sottosistema PCIe. Formalizzato AHCI come step dedicato.

## File toccati
- docs/superpowers/roadmap-rust-os.md
