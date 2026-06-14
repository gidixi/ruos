# 522 — disabilita le animazioni egui (dissolvenza hover) → hover menu fluido

**Data:** 2026-06-14

## Cosa
Residuo di "hover menu fa schifo" dopo il fix damage (521): le **animazioni egui**
(dissolvenza di hover/selezione/tooltip/apertura-menu). Con `animation_time` di
default (0.083 s) ogni transizione è un fade: egui chiama `request_repaint()` ad OGNI
frame finché il fade non termina → la shell full-screen rifà la pipeline completa
(`ctx.run` + tessellate + `ship_mesh` + decode + `plan_damage`) in continuo durante
l'hover, anche se il damage raster è piccolo (521). Costo sostenuto per ~83 ms ad
ogni cambio di hover.

Fix: in `WindowState::new` (ruos-window) azzero `animation_time`
(`ctx.style_mut(|s| s.animation_time = 0.0)`). Le transizioni hover/menu diventano
ISTANTANEE → 1 render per cambio stato, niente repaint continuo. Vale per tutte le
app finestra. Su un OS senza GPU è anche più reattivo (niente lag di fade).

## Perché
Il fix 521 (damage parziale) aveva tolto il re-raster full-screen, ma la dissolvenza
teneva la finestra a ridisegnare in continuo (frame_all + ship + decode + plan_damage
ogni frame del fade). Azzerare le animazioni elimina la sorgente di repaint
continuo → l'hover costa solo i pochi render dei cambi di stato effettivi.

Solo config di stile egui in ruos-window (on-device); NON tocca il rasterizzatore
né la bit-identità (cross-check invariato — la mesh di test non ha animazioni in
volo).

## File toccati
- ruos-desktop/crates/ruos-window/src/lib.rs (WindowState::new: animation_time = 0.0)
