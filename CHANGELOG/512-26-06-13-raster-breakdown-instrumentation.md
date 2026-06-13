# 512 — wm-fps: breakdown di raster_meshes (decode/plan/dispatch/clone)

**Data:** 2026-06-13

## Cosa
Su HW REALE (bare-metal, NON la VBox-NEM) System Monitor rende a ~3fps con
`rast=320ms` e `hlt=8ms` → work-bound, il raster è il vero collo. Per localizzare
DOVE nei 320ms, strumentato `raster_meshes` (gated `wm-fps`) cronometrando le 4
sotto-fasi per finestra dirty: **decode** (wire→Vec), **plan** (plan_damage, hash
O(prims)), **dispatch** (dispatch_raster = raster + join SMP), **clone**
(canvas.to_vec→pixels). Accumulate in statiche (RP_DEC/PLN/DSP/CLN), mediate per
report e mostrate sulla **riga 2 dell'overlay** (`d:.. p:.. r:.. c:..ms b:N`) + riga
log `wmfps2`. `RP_LAST` packa (dmg_rows<<16 | n_bands) per vedere se parallelizza.

Rimosso lo spam `binfo "mesh render"` (ogni frame).

PRIMA scoperta (QEMU idle, shell): `decode=31us plan=1453us dispatch=0us clone=0us`
→ **plan_damage domina** (97%) quando il damage è piccolo. Da confermare su HW reale
con SM (damage grande → dispatch in gioco).

## Perché
Il fix #510 (soglia dispatch_raster) non ha cambiato i numeri perché il collo non è
il fan-out. Localizzare la sotto-fase dominante (plan O(prims)? il raster? il clone?)
prima di ottimizzare — anti-thrashing.

## File toccati
- kernel/src/wasm/wt/wm.rs (statiche RP_*, timing in raster_meshes, RP_LAST in
  dispatch_raster, report wmfps2 + overlay riga 2)
