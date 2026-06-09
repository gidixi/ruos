# 364 — terminal: scroll input — wheel + scrollbar overlay + new-output badge

**Data:** 2026-06-09

## Cosa
Aggiunti in `Terminal::ui()` tre blocchi per l'input di scroll (Task 5):

1. **Mouse wheel → view_offset**: `smooth_scroll_delta.y` convertito in righe; clamp
   su `[0, sb]`; se torna a fondo (`view_offset == 0`) spegne `new_output`.
2. **Scrollbar overlay** sul bordo destro (8 px, non ruba colonne al grid):
   track semitrasparente + thumb draggable. Drag/click calcola la frazione Y →
   `view_offset`; la posizione del thumb è invertita (offset 0 = fondo = thumb in
   basso; offset max = thumb in alto).
3. **Badge "▼"** (bottom-right) visibile quando `new_output && view_offset > 0`:
   sfondo blu semitrasparente, click → salta a fondo e spegne l'indicatore.

## Perché
Completamento del feature terminal-scrollback: senza input scroll l'utente non
poteva interagire con l'offset introdotto nei task precedenti.

## File toccati
- ruos-desktop/crates/gui-core/src/desktop/apps/terminal.rs
