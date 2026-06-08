# 344 — AHCI: acquire_atapi_port

**Data:** 2026-06-08

## Cosa
Helper `ahci::acquire_atapi_port()` che porta su la prima porta ATAPI (CD-ROM) del boot HBA.

## Perché
La fase storage lo userà per montare `/bin` dal CD live (ISO9660/ATAPI).

## File toccati
- kernel/src/ahci/mod.rs
- CHANGELOG/344-26-06-08-ahci-acquire-atapi-port.md
