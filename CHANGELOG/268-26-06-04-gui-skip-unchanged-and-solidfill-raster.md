# 268 — Desktop egui: salta frame invariati + raster fill a campione singolo

**Data:** 2026-06-04

## Cosa
Due ottimizzazioni in `gui-core` per ridurre lo "scatti" residuo del desktop
(dopo che il fix del clock monotonico [CHANGELOG 267] aveva tolto il flicker ma
non la pesantezza per-frame). Nessun cambiamento visivo.

1. **Skip-when-unchanged** (`lib.rs`): a ogni frame egui ri-emette TUTTA la scena
   (immediate mode), quindi muovere il mouse su aree statiche ri-rasterizzava
   1280×800 per niente. Ora `frame()` calcola un hash FNV-1a della scena
   tassellata (clip rect + vertici + indici + texture id + size) e, se è
   identico al frame presentato l'ultima volta e nessuna texture è cambiata,
   **salta raster + blit** e ritorna. Il cursore è disegnato dal kernel a parte,
   quindi un frame saltato mostra comunque il movimento del mouse. Hashare
   qualche migliaio di vertici costa microsecondi vs i ~ms del raster full-screen.

2. **Fast-path fill a campione costante** (`raster.rs`): egui usa `WHITE_UV` (uv
   identici sui 3 vertici) per tutto ciò che non è testo (wallpaper, pannelli,
   cornici, separatori). In quel caso il texel campionato è costante sul
   triangolo → ora si campiona **una volta** prima del loop pixel invece del
   bilinear per-pixel. Il testo (uv variabili sull'atlante font) tiene il path
   per-pixel. Estratto `sample_bilinear()` condiviso dai due rami → risultato
   identico al pixel, solo più veloce (il wallpaper full-screen è il fill più
   grande e ne beneficia di più).

## Perché
Feedback utente: dopo 267 "ancora lento, va a scatti, molto meglio di prima". Il
costo dominante per-frame era il raster software full-scene + blit ripetuto anche
quando nulla cambiava (es. mouse mosso sul desktop).

## Verifica
- `cargo test -p gui-core` → 7/7 ok (raster: solid rect, alpha blend, gradient —
  esercitano il path uv-costante → pixel identici).
- `gui.cwasm` ricompilato + ISO ribuildata.

## Resta (onesto)
Durante hover/drag la scena cambia ogni frame → si ri-rasterizza comunque TUTTO
lo schermo (anche il wallpaper statico). Fluidità piena in interazione richiede
**rendering a dirty-rect** (ri-rasterizzare/blittare solo la regione cambiata) o
edge-function incrementali nel rasterizzatore — follow-up più grosso, separato.

## File toccati
- ruos-desktop/gui-core/src/lib.rs (scene_hash + skip-when-unchanged)
- ruos-desktop/gui-core/src/raster.rs (sample_bilinear estratto + fast-path uv-costante)
