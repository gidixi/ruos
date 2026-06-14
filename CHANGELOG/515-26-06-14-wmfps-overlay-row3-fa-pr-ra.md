# 515 — wm-fps: overlay riga 3 (frame_all / present / raster)

**Data:** 2026-06-14

## Cosa
Dopo i fix 513+514 il `raster_meshes` è crollato su HW reale (C: r 479→55ms, B
274→150ms) ma `iter` non è sceso proporzionalmente → il collo si è **spostato** su
una fase NON-raster. L'overlay mostrava solo `iter` e `hlt` (riga 1) + il breakdown
raster (riga 2), non `frame_all` né `present` (visibili solo nella riga di log
`wmfps`, che richiede netconsole).

Aggiunta **riga 3** all'overlay: `fa:{frame_all}ms pr:{present}ms ra:{raster}ms`.
`disp_fa`/`disp_pr`/`disp_ra` erano già calcolati (servivano solo al log) → ora
disegnati. Box overlay allargato a 3 righe. `iter ≈ fa + ra + pr + hlt` → localizza
i ~340ms residui di C tra tessellazione app (frame_all, sotto Wasmtime) e
compositing (present, `compose.rs`, NON toccato dai fix raster).

## Perché
Solo diagnostico, zero logica cambiata. Evita un terzo fix a caso: i numeri B/C
dicono che raster è risolto e il residuo è altrove; la riga 3 dice DOVE (frame_all
vs present) così il prossimo fix è mirato.

## File toccati
- kernel/src/wasm/wt/wm.rs (overlay: ov_h a 3 righe + riga l3 fa/pr/ra)
