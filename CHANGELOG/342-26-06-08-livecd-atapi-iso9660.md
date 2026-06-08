# 342 — Spec live-CD: /bin overlay da ISO9660 via ATAPI

**Data:** 2026-06-08

## Cosa
Spec di design per spostare i bin userspace off-boot: Limine carica solo il kernel,
`/bin` viene montato e letto on-demand dal CD (filesystem ISO9660, driver ATAPI su
AHCI), niente più moduli Limine pre-caricati in RAM. Riprende il lavoro accantonato
del branch `livecd`. Primo taglio = solo live-CD; SSD installato coperto dal fallback
legacy `mount_all()`.

## Perché
Oggi ~80 bin CLI + 5 app .cwasm (~45 MB) sono moduli Limine: Limine li carica in RAM
(HHDM) e `mount_all()` li ricopia in tmpfs → doppia RAM e boot che pre-carica tutto.
Boot più elegante + RAM più bassa caricando i bin dal medium live on-demand.

## File toccati
- docs/superpowers/specs/2026-06-08-livecd-atapi-iso9660-design.md
- CHANGELOG/342-26-06-08-livecd-atapi-iso9660.md
