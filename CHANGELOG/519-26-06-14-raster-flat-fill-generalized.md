# 519 — raster_tri: fast-path flat generalizzato (anche translucido)

**Data:** 2026-06-14

## Cosa
Ottimizzazione del costo per-pixel di `raster_tri` per i fill PIATTI (colore
costante sui 3 vertici), che in System Monitor sono grandi: pannelli "glass"
`rgba(30,32,40,150)`, sfondo del grafico `rgba(12,14,20,120)`, righe della tabella.
Prima solo i fill flat OPACHI prendevano la fast-path (scrittura costante); i flat
TRASLUCIDI passavano per lo slow path completo (pesi baricentrici + interpolazione
4 canali + sample texel + normalizzazione frag), pur avendo frammento e `inv`
COSTANTI su tutto il triangolo.

Generalizzato: per ogni triangolo flat con texel costante (fill, non testo)
precalcolo UNA volta il frammento `(fr,fg,fb,fa)` e `inv`:
- **opaco** (`inv==0`) → l'uscita è una costante → `flat_const`, `put` diretto (nessun
  accesso al dst, nessun calcolo per-pixel);
- **translucido** (`inv>0`) → `flat_blend`: nel loop resta SOLO il blend OVER
  (`fr + dst*inv`), che dipende dal dst; saltati pesi/interp/sample/normalizza.

Il testo (uv variabili → colore e texel per-pixel) resta lo slow path completo.
Triangolo del tutto trasparente (`fa<=0`) → ritorno anticipato (niente loop).

Applicato **identico** a `ruos-raster` (kernel) e mirror `gui-core/raster.rs`.
≤1 LSB dal per-pixel (perché `cr=c0·Σw≈c0` con Σw≈1), ma il cross-check resta
byte-esatto perché entrambe le crate precalcolano allo stesso modo. Verifica host:
ruos-raster 13 + `crosscheck` byte-identico verdi (incl. `alpha_blends` su rect
translucido), gui-core 44 verdi.

## Perché
Dopo i fix precedenti il collo residuo degli fps di System Monitor è il raster del
suo contenuto: i fill flat translucidi (pannelli + sfondo grafico) coprono gran
parte della finestra e pagavano l'intero per-pixel inutilmente. Per un triangolo
flat tutto è costante tranne il blend → calcolarlo una volta dimezza ~il per-pixel
su quelle aree, senza toccare la correttezza (cross-check resta l'oracolo).

## File toccati
- ruos-raster/src/lib.rs (raster_tri: flat_const + flat_blend precalcolati; loop)
- ruos-desktop/crates/gui-core/src/raster.rs (mirror identico)
