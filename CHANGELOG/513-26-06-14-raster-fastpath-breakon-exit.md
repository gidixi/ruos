# 513 — raster: fast-path opaco piatto + break-on-exit (collo perf HW reale)

**Data:** 2026-06-14

## Cosa
Ottimizzato il loop per-pixel di `raster_tri` — il collo perf misurato su HW
reale: System Monitor renderizzava a ~3fps con `r` (dispatch raster) = 274ms su
**un** frame, b=7 bande (parallelizza già), tutto il resto (decode/plan/clone)
≈0. Due finestre dirty (SM + hover menu) → r=491ms. Localizzato leggendo il loop:
era una rasterizzazione software non ottimizzata.

Due ottimizzazioni, entrambe **bit-identiche** allo slow path (provate + verificate
dai test, nessuna regressione visiva possibile):

1. **Fast path FLAT-OPAQUE-SOLID.** egui emette wallpaper/pannelli/cornici/barre
   come triangoli a colore costante sui 3 vertici, texel bianco-opaco, alpha 255.
   In quel caso l'uscita è la COSTANTE `color`: `cr = c·(w0+w1+w2) ≈ c` (errore
   ULP < 0.5 assorbito da `round()`), `inv = 1−255/255 = 0` esatto → `out = color`
   byte per byte. Si salta interpolazione + 4 divisioni `/255` + 4 `round()`
   software (no_std) + sample texel per il caso dominante (overdraw opaco).

2. **break-on-exit per scanline.** La copertura di un triangolo convesso su una
   riga è un intervallo contiguo: una volta entrati e poi usciti dal triangolo, il
   resto della riga è fuori → `break` invece di testare 3× `edge()` (f64) su ogni
   pixel del margine destro del bounding box. Pixel set INVARIATO (gli AA di egui
   sono triangoli sottili con bbox ≫ area → tanti `edge()` sprecati).

Hoisted anche `c0/c1/c2 = to_le_bytes()` fuori dal loop (costanti per triangolo).

Applicato **identico** sia a `ruos-raster` (kernel) sia al mirror
`gui-core/raster.rs` (anteprima PC + riferimento del cross-check). Verifica host:
ruos-raster 13 test + `crosscheck` (egui→gui-core vs wire→ruos-raster
byte-identico) verdi; gui-core 44 test verdi (incl. `banded_matches_serial`).

## Perché
Il fix #510 (soglia dispatch_raster) e gli #511/#512 (strumentazione) avevano
escluso fan-out, plan_damage, clone e timer. Il collo è il fill grezzo dei
triangoli (r=274ms = ~2-3µs/px effettivi = ~100× troppo lento per un rasterizer
sw, amplificato dall'overdraw egui). Fast-path opaco + meno `edge()` sprecati =
attacco diretto al fill senza toccare la correttezza (bit-identico → cross-check
resta l'oracolo). Da misurare su HW reale (TCG non mostra perf).

## File toccati
- ruos-raster/src/lib.rs (raster_tri: fast_const + break-on-exit + hoist colori)
- ruos-desktop/crates/gui-core/src/raster.rs (mirror identico)
