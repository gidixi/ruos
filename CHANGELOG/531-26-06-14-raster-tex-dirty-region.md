# 531 — raster: damage per-regione-atlante + wallpaper su layer fisso (fix freeze menu)

**Data:** 2026-06-14

## Cosa

Fix COMPOSTO delle due root-cause del freeze del menu.

### 1. Damage per-regione-atlante (al posto del flag globale)

Sostituito il flag globale `tex_dirty: bool` + hack `white_only` con
**dirty-region per-texture** sia in `ruos-raster` (kernel) sia in `gui-core`
(reference + path pixel legacy), tenuti 1:1.

- `set_texture` ora registra il **sotto-rettangolo patchato** dell'atlante
  (`pos=Some` → rect del patch; `pos=None` → intero atlante; `free` in gui-core →
  rect vuoto = texture liberata) in `tex_dirty: Vec<(tex_id, IRect)>`.
- `prim_meta` calcola la **uv-bbox** della primitiva (min/max uv sui vertici
  referenziati) + `tex_id`, al posto di `white_only`.
- `plan_damage`: per ogni regione patchata, danneggia SOLO i prim che la
  **campionano** (uv-bbox mappata in pixel dell'atlante ∩ rettangolo patchato).
  I fill solidi campionano (0,0) → fuori da ogni patch di glifo → risparmiati
  senza casi speciali. Texture liberata → tutti i prim che la usavano.

### 2. Wallpaper sul layer di sfondo a rect fisso (wiring mancante)

`shell_chrome` disegnava il wallpaper dentro un `CentralPanel` con
`wallpaper::paint(ui)` (`ui.max_rect()`): il max_rect del pannello jitterava
sub-pixel quando l'hover cambiava il layout della top-bar → i vertici f32 del
wallpaper cambiavano → l'hash per-prim del raster kernel cambiava → re-raster
FULL-SCREEN del wallpaper ad ogni frame di hover. `wallpaper::paint_bg(ctx)`
(layer `background()` a `ctx.screen_rect()`, byte-stabile per risoluzione) esisteva
già ma NON era cablato. Ora `shell_chrome` lo usa → mesh stabile → hash stabile →
wallpaper mai ri-rasterizzato. **Va chiamato PRIMA dei pannelli**: pannelli egui e
wallpaper condividono il layer `background()` (z = ordine d'inserimento), quindi
disegnando il wallpaper per primo la top-bar (dipinta dopo) resta SOPRA. (Disegnarlo
dopo nascondeva la barra.)

## Perché

Root-cause del freeze del menu (misure HW, changelog 528-530): durante
l'interazione il loop crollava a ~8 it/s con raster full-screen ~300ms/frame. Due
cause distinte, entrambe portano a FULL damage:
1. egui aggiunge glifi lazy all'atlante font → patch texture → il vecchio
   `tex_dirty`→full (o danno di TUTTI i prim non-white) ri-rasterizzava quasi tutto.
   Ora un patch di glifo tocca solo lo slot del glifo nuovo → danno quello slot
   (minuscolo). È come tracciano la texture-dirty i renderer veri (non un flag
   globale che invalida tutto).
2. il wallpaper full-screen, in un CentralPanel, cambiava hash per jitter sub-pixel
   dei vertici f32 → re-raster full-screen su ogni hover. Ora geometria fissa.

## File toccati

- ruos-raster/src/lib.rs
- ruos-desktop/crates/gui-core/src/raster.rs
- ruos-desktop/crates/gui-core/src/desktop/shell.rs
