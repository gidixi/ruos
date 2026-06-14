# 517 — fluidità: render-on-input + cap a divider 2 + System Monitor 10 Hz

**Data:** 2026-06-14

## Cosa
Il cap 516 (1 render ogni 5 chiamate, con coalescing dell'input) aveva reso il
desktop reattivo e il mouse fluido (frame_all→0, iter 220→46ms), ma:
- **coalescing dell'input** = fino a ~5 tick di latenza (~230ms) su click/scroll/
  hover/tab → le interazioni sembravano "a scatti";
- **System Monitor a ~4fps** (divider 5 al loop ~22 Hz = 4.4 render/s < 5 Hz del
  dato) → saltava campioni → grafico a scatti.

Indagine: la **damage di SM è intrinsecamente full-window** (grafico a tutta
larghezza + barre core + tabella processi che si RI-ORDINA ogni campione → i prim
che cambiano coprono tutta l'altezza → b=6-7). Damage piccola non fattibile senza
ridisegnare la dashboard. Le leve vere per la fluidità sono la **latenza input** e
il **refresh rate**, non la damage. Quindi:

1. **render-on-input** (`ruos-window::repaint_gate`): se c'è input in coda
   renderizza SUBITO (nessun coalescing/latenza); solo le animazioni SENZA input
   restano capate. Interazioni immediate.
2. **`REPAINT_CAP_DIVIDER` 5→2**: animazioni a ~metà del rate del loop, sopra il
   rate dati → nessun campione saltato → grafico fluido; resta un tick "cheap" di
   headroom tra i render (mouse servito).
3. **System Monitor `SAMPLE_CS` 20→10** (5 Hz→10 Hz): il dato del grafico aggiorna
   il doppio più spesso → movimento più fluido.

Il cursore è disegnato dal compositor (software cursor) → resta fluido a prescindere
dal rate di refresh delle app.

## Perché
"Deve essere fluido, non a scatti." Gli scatti residui erano la latenza di
coalescing dell'input (interazioni) e il refresh sotto il rate dati (grafico). La
damage piccola non è praticabile per la dashboard densa di SM → ho puntato su
input-latency + refresh, che è ciò che dà la fluidità percepita. `DIVIDER` resta il
knob: se il mouse dovesse perdere headroom, alzarlo.

## File toccati
- ruos-desktop/crates/ruos-window/src/lib.rs (repaint_gate render-on-input; DIVIDER 5→2)
- ruos-desktop/crates/gui-core/src/desktop/apps/system.rs (SAMPLE_CS 20→10)
