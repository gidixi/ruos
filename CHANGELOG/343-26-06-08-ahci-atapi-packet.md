# 100 — AHCI: ATAPI PACKET read path + 2048B BlockDevice

**Data:** 2026-06-08

## Cosa
Esteso `kernel/src/ahci/port.rs` per parlare con un CD-ROM ATAPI sulla porta
AHCI tramite il comando ATA PACKET (DMA-in), leggendo blocchi logici da 2048 B.

- Nuove costanti: `SIG_ATAPI` (0xEB14_0101), `ATA_PACKET` (0xA0),
  `PACKET_FEATURE_DMA`, `CH_FLAG_ATAPI` (bit A del command header), `ATAPI_BLOCK`.
- Nuovo campo `pub is_atapi: bool` su `AhciPort`.
- `bringup` ora accetta sia la firma SATA che quella ATAPI; per ATAPI usa
  READ CAPACITY(10) invece di IDENTIFY per ricavare il numero di blocchi.
- Nuovi metodi `issue_atapi` (PACKET DMA con CDB nel campo `acmd` della Command
  Table) e `atapi_read_capacity`.
- `impl BlockDevice`: `block_size` dinamico (2048 ATAPI / 512 SATA); `read_blocks`
  con ramo ATAPI via READ(10); `write_blocks` rifiuta i device ATAPI (CD read-only).

## Perché
Task 2 della feature live-CD: il CD di boot è un device ATAPI sul controller AHCI
q35; i task successivi monteranno ISO9660 sopra questo BlockDevice.

## File toccati
- kernel/src/ahci/port.rs
