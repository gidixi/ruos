# 526 — riattivato render-on-input (ora che i render sono economici)

**Data:** 2026-06-14

## Cosa
Numeri overlay su HW (hover icona menu, build 525): `iter:14ms hlt:9ms` (67 Hz, loop
**idle al 64%**), `ra:4ms` (raster minuscolo), `fa:0`, `pr:10ms`, `fps:2`. → Il
sistema NON è più saturo: i fix 523 (skip ship se mesh invariata) + 524 (damage per
contenuto, niente più full-screen) hanno reso ogni render economico.

Quindi lo "scatta" residuo del menu NON è compute: è la **latenza di input** del
coalescing (`REPAINT_CAP_DIVIDER=5` → ~70 ms tra movimento mouse e aggiornamento
highlight) + il present a soli 2 fps (aggiorna solo sui cambi di mesh). Riattivato il
**render-on-input** in `repaint_gate`: con input in coda si renderizza SUBITO →
highlight segue il mouse senza latenza, screen aggiorna ad ogni movimento.

Sicuro ORA (era la regressione del 517): la tempesta del 517 era il re-raster
full-screen da ~50 ms ad ogni evento mouse; il 524 (damage per contenuto) l'ha
ridotto a ~4 ms, quindi render-on-input non satura più il loop (resta veloce → niente
fame del pump cursore). Senza input l'animazione resta capata a 1/DIVIDER (idle).

## Perché
La diagnosi finale (dai numeri, non a tentativi): compute risolto da 523+524, il
residuo era latenza/feel. Render-on-input toglie i ~70 ms di ritardo → menu reattivo.
Reintrodotto solo dopo che 523+524 hanno tolto la causa della tempesta originale.

## File toccati
- ruos-desktop/crates/ruos-window/src/lib.rs (repaint_gate: render-on-input se eventi
  in coda)
