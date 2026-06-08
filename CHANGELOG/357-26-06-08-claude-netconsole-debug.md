# 357 — CLAUDE.md: netconsole come canale di debug HW reale

**Data:** 2026-06-08

## Cosa
Aggiunta a `CLAUDE.md` (sezione Ambiente di build) una voce che istruisce a
usare **netconsole** come canale di log preferito per il debug su hardware reale
senza seriale: build con `CARGO_FEATURES=netconsole`, ricevitore host
`tools/netconsole-rx/`, stream live + backlog da T+0; nota sul limite pre-rete
(usare framebuffer per hang pre-T+3s) e sul requisito NIC supportato.

## Perché
Meccanismo di debug importante e ricorrente per bare-metal: va in CLAUDE.md così
le sessioni future lo usano d'ufficio invece di reinventarlo.

## File toccati
- CLAUDE.md
