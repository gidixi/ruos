# 360 — Grid::view_row + scrollback_len (scrollback read access)

**Data:** 2026-06-09

## Cosa
Aggiunti due accessor read-only a `Grid` in `ruos-desktop`:
- `scrollback_len() -> usize`: ritorna il numero di righe attualmente in scrollback (0..=SCROLLBACK_LINES).
- `view_row(view_offset: usize, screen_row: u16) -> &[Cell]`: compone scrollback (in cima) + griglia viva (sotto) per supportare la vista arretrata del terminale. `view_offset=0` equivale alla vista live; valori maggiori arretrano nella storia.

Aggiunti 5 test `#[cfg(test)]` in fondo a `grid.rs` (TDD: tutti e 5 passano).

## Perché
Task 1 della feature terminal-scrollback: esporre i dati di scrollback già presenti in `Grid` (il buffer `VecDeque<Vec<Cell>>` era già popolato da `scroll_up()`) affinché un task successivo possa renderizzare il contenuto scrollato.

## File toccati
- crates/gui-core/src/desktop/apps/term/grid.rs
