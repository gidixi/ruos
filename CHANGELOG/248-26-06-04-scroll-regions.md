# 248 — Scroll regions (DECSTBM)

**Data:** 2026-06-04

## Cosa
Aggiunto supporto alle scroll region DECSTBM (`ESC [ top ; bot r`).
- `Grid`: nuovi campi `scroll_top`/`scroll_bot` (inizializzati a 0/rows-1);
  `set_scroll_region()` setter; `scroll_up()` riscritto per scrollare solo la
  banda `[scroll_top, scroll_bot]`; `newline()` riscritto per confrontare con
  `scroll_bot` invece di `rows`; `clear()` resetta la regione a full-screen.
- `fb.rs`: arm `'r'` in `csi_dispatch` per DECSTBM.
- `engine_test.rs`: T42-T43 verificano che newline a fondo regione scrolla solo
  la banda e lascia le righe esterne intatte.

## Perché
Task 4 del piano terminal-engine-vt (Plan 3). Prerequisito per applicazioni che
usano split-screen o status bar fisse (e.g. nano, htop).

## File toccati
- kernel/src/console/grid.rs
- kernel/src/console/fb.rs
- kernel/src/console/engine_test.rs
- CHANGELOG/248-26-06-04-scroll-regions.md
