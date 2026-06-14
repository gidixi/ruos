# 520 — revert render-on-input → coalescing (hover-stutter regression)

**Data:** 2026-06-14

## Cosa
Il `render-on-input` introdotto in 517 (ridisegna SUBITO quando c'è input) causava
una REGRESSIONE: avvicinandosi al menu app, il mouse genera eventi ad ogni tick →
la shell full-screen ridisegnava il frame egui completo ad OGNI evento → loop
saturo, nessun tick "cheap", e il pump del cursore (518, gira solo negli spin-wait
inattivi dei join) si affamava → **tutto scattava**, cursore incluso.

Revert mirato: `repaint_gate` torna al **coalescing puro** (1 render ogni
`REPAINT_CAP_DIVIDER` chiamate, input accumulato e consegnato al prossimo render) e
`REPAINT_CAP_DIVIDER` 2→5 (com'era nel 516, che era fluido). Così i tick saltati
tornano subito (~0 lavoro) → il loop torna a `hlt` → il pump del cursore gira →
puntatore fluido sotto carico; e l'hover non scatena più una raffica di render.

Tenuti i miglioramenti buoni: cursore disaccoppiato (518), raster flat generalizzato
(519), System Monitor 10 Hz (517). Latenza input ≤ 5 tick (impercettibile, e il
cursore è disaccoppiato).

## Perché
Disciplina anti-thrashing: isolato il singolo cambiamento colpevole (render-on-input)
invece di impilarne altri. Il coalescing + cursore disaccoppiato è la combinazione
corretta: rate di render capato (no saturazione) + cursore fluido (pump nei tick
liberi e nei join).

## File toccati
- ruos-desktop/crates/ruos-window/src/lib.rs (repaint_gate: rimosso render-on-input;
  REPAINT_CAP_DIVIDER 2→5)
