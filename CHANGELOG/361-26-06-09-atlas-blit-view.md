# 361 — atlas::blit_view — repaint pieno composto

**Data:** 2026-06-09

## Cosa
Aggiunta la funzione pubblica `blit_view` in `crates/gui-core/src/desktop/apps/term/atlas.rs`.
Compone scrollback + griglia viva tramite `grid.view_row` e disegna TUTTE le celle
(non solo le dirty), consentendo il repaint corretto della finestra visibile a qualsiasi
`view_offset` (0 = live). Le righe di scrollback più corte delle colonne correnti
(post-reflow) vengono paddate con celle blank via `slice.get().unwrap_or(blank)`.

Aggiunti 3 test TDD nel `mod tests` esistente di `atlas.rs`:
- `blit_view_offset0_eq_dirty` — a offset 0 il risultato è identico a `blit_dirty` su mark-all
- `blit_view_shows_scrollback` — a offset 1 viene disegnato il glifo dallo scrollback
- `blit_view_pads_short_row_no_panic` — nessun panic dopo reflow con riga scrollback più corta

## Perché
Task 2 della feature terminal-scrollback: il renderer deve poter comporre
e visualizzare le righe dello scrollback senza limitarsi alle sole celle dirty.

## File toccati
- ruos-desktop/crates/gui-core/src/desktop/apps/term/atlas.rs
