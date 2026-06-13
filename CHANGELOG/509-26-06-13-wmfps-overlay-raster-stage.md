# 509 — wm-fps overlay: mostra lo stadio raster (per la misura su HW)

**Data:** 2026-06-13

## Cosa
L'overlay a schermo `wm-fps` mostrava `rendering: X ms / blit: Y ms`, dove
`rendering` = `frame_all`. In mesh-mode `frame_all` è solo tessellation+encode
(leggera) e il costo vero — la **rasterizzazione kernel-side** — non era a schermo
(solo nella riga di LOG `wmfps`). Su HW reale (senza seriale) si leggerebbe un
`rendering` fuorviante senza il raster.

Riga 2 dell'overlay ora: `tess:{} rast:{} blit:{} ms` (tessellation = frame_all,
raster = stadio kernel `dispatch_raster`, blit = composite+present). `disp_ra`
aggiunto allo stato + popolato dal report 1s. La riga LOG `wmfps` (µs, più precisa
via netconsole) era già stata estesa con `raster avg`.

## Perché
Rendere la misura su HW reale significativa: in mesh-mode il numero che conta è il
raster kernel, non `frame_all`. ms a schermo è grossolano su HW veloce → per i µs
usare la riga `wmfps` via netconsole.

## File toccati
- kernel/src/wasm/wt/wm.rs (disp_ra + overlay riga 2)
