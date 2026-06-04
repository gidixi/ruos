# 266 — Puntatore mouse software nel desktop egui

**Data:** 2026-06-04

## Cosa
Aggiunto un cursore del mouse visibile nel desktop egui (`crate::gfx`). Una
freccia 12×19 disegnata direttamente sul framebuffer in GUI mode: il mouse
cliccava ma non si vedeva dove fosse.

Caratteristiche:
- Sprite freccia (outline nero + riempimento bianco, pixel trasparenti saltati).
- **Responsivo:** ridisegnato a ogni movimento del mouse in `fold_mouse()` —
  NON forza un re-render egui (che è full-screen e lento).
- **Senza scia:** salva/ripristina il background sotto lo sprite
  (`cursor_erase` / `cursor_paint`).
- Ricomposto sopra ogni blit full-frame (`cursor_after_blit`), che altrimenti lo
  cancellerebbe.
- Centrato all'ingresso in GUI mode (`enter()`); cancellato all'uscita
  (`leave()`).
- Assume framebuffer 32-bpp; i pixel nero/bianco sono RGB/BGR-agnostici.

## Perché
Usabilità: senza un puntatore visibile il desktop era inutilizzabile (clic alla
cieca). Disegnarlo nel kernel lo rende fluido indipendentemente dal costo del
re-render egui.

## File toccati
- kernel/src/gfx/mod.rs (modulo cursore + hook in enter/leave/blit/fold_mouse)
