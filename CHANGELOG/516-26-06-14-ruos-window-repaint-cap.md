# 516 — ruos-window: repaint cap ~20fps (frame_all = vero collo)

**Data:** 2026-06-14

## Cosa
Dopo i fix raster (513+514) il collo si è spostato su **frame_all**: ogni app
sveglia rifà il frame egui COMPLETO (`ctx.run` + tessellate + `ship_mesh`) ad OGNI
giro del loop del compositor (~100/s), anche quando nulla cambia. Verificato leggendo
il codice: `present`/`compose_window` restituisce un puntatore alla surface (no
clone) e `composite_band` è memcpy per riga → present è cheap (~15ms), NON il collo.
Il costo è il frame egui in WASM (~50ms/app) ripetuto in continuo + il raster della
finestra ad ogni ship → loop al 100% senza dormire (hlt=5ms) → core saturi → mouse/
USB in fame → lag (la causa di "core schizzano / mouse lagga / fps bassi").

Aggiunto un **repaint cap** in `ruos-window` (`frame_once` + `frame_once_bare`):
ridisegna al massimo 1 chiamata su `REPAINT_CAP_DIVIDER=5` (≈20 fps a 100 Hz). Le
chiamate saltate tornano subito (~0 lavoro: niente `ctx.run`/tessellate/ship) → il
kernel tiene la surface in cache e non ri-rasterizza. Gli eventi input drenati nei
tick saltati sono **accumulati** (`EVENT_ACC`) e consegnati tutti al prossimo render
(coalescing) → niente input perso, latenza ≤ ~50 ms. Il cursore lo disegna il
compositor (software cursor) → resta fluido a prescindere dal rate dell'app.

Nota: il mesh-path di `frame_once` shippava la mesh INCONDIZIONATAMENTE ogni frame
(il pixel-path aveva il commit-on-damage); il cap copre anche quel buco riducendo la
frequenza di ship.

## Perché
La frequenza di ridisegno era illimitata (100/s). System Monitor chiama
`stay_awake()` ogni frame → mai dorme; l'hover tiene la shell sveglia. Cap + sleep
tra i frame = core liberi, mouse fluido, le altre app respirano. Approvato
dall'utente (~20 fps). Le app pesanti (SM ~200ms/frame) restano work-bound a ~4-5fps
ma ora con tick "cheap" di headroom in mezzo (mouse servito); le app leggere vanno
a 20fps pieni. Il costo del singolo frame pesante (raster slow-path dei grafici /
area ridisegnata) resta come ottimizzazione successiva.

## File toccati
- ruos-desktop/crates/ruos-window/src/lib.rs (REPAINT_CAP_DIVIDER + FRAME_TICK +
  EVENT_ACC + repaint_gate; applicato in frame_once e frame_once_bare)
