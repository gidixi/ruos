# 103 — Roadmap: riordino step 14–18 per dipendenze + valore north-star

**Data:** 2026-05-29

## Cosa
Riordinati gli step rimanenti dopo analisi dipendenze. Nuovo ordine:

| Step | Titolo | Prima |
|------|--------|-------|
| 13 | PCI/ECAM | 13 |
| 14 | Networking (virtio-net + CSPRNG) | 15 |
| 15 | AHCI + FAT | 18 |
| 16 | SSH server | 16 |
| 17 | Mouse PS/2 + rlvgl | 14 |
| 18 | SMP / multi-CPU | 17 |

Aggiornati tutti i cross-reference (CSPRNG→14, AHCI prereq/deps, "FAT/AHCI allo
Step 15") e riscritto il diagramma di dipendenza con catena critica north-star
(PCI→Net→SSH) + rami AHCI / mouse / SMP. Aggiunta nota DMA: networking costruisce
l'allocator DMA, AHCI lo riusa (perciò contigui).

## Perché
La numerazione precedente spezzava la catena critica: PCI (13) → mouse scollegato
(14) → consumatore-di-PCI networking (15). Riordino motivato da:
- virtio-net e AHCI sono entrambi device PCIe → dipendono dallo Step 13.
- north-star = accesso remoto → catena PCI→Net→SSH va contigua e prioritaria.
- networking introduce l'allocator DMA che AHCI riusa → AHCI subito dopo
  (scelta utente: "AHCI subito dopo Net").
- mouse/rlvgl = foglia che dipende solo da step già fatti, zero dipendenti →
  spostabile dopo SSH.
- SMP = trasversale, alto rischio, ultimo.

## File toccati
- docs/superpowers/roadmap-rust-os.md
