# 525 — icona menu vettoriale (test ipotesi utente: texture vs vettori)

**Data:** 2026-06-14

## Cosa
Su richiesta dell'utente (l'hover dell'icona menu resta pesante mentre i pulsanti
vettoriali power/reboot/console a destra vanno bene): sostituita l'icona del launcher
da **texture** (`egui::Image` del PNG menu-dots, campionata bilineare per-pixel) a
**vettori** (hamburger = 3 segmenti stroke, fill flat nel rasterizzatore), coerente
coi pulsanti del pannello.

- `icons::paint_hamburger` — 3 linee orizzontali centrate, stroke.
- `images::vector_menu` — gemello di `icon_menu` ma disegna l'hamburger a vettori
  (niente texture) e mantiene il popup egui (`popup_below_widget`).
- `shell_chrome` usa `vector_menu` al posto del ramo texture/glifo.

`icon_menu` (texture) resta per chi lo usa (anteprima PC); il test
`injected_menu_icon_renders_in_launcher` (anteprima) NON è toccato → 44 gui-core
verdi.

## Perché
Esperimento mirato per isolare se il costo dell'hover menu sia la texture/bilinear
dell'icona (ipotesi dell'utente, supportata dal fatto che i pulsanti vettoriali a
destra hanno hover fluido). Se l'hover diventa fluido → era l'icona texture; se no →
il collo è altrove (e servono i numeri overlay per localizzarlo). Cambiamento solo di
rendering UI, nessun impatto su raster/bit-identità.

## File toccati
- ruos-desktop/crates/gui-core/src/desktop/icons.rs (paint_hamburger)
- ruos-desktop/crates/gui-core/src/images.rs (vector_menu)
- ruos-desktop/crates/gui-core/src/desktop/shell.rs (shell_chrome usa vector_menu)
