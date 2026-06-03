# 232 — render::flush — composizione celle dirty → pixel + blit

**Data:** 2026-06-03

## Cosa
Aggiunto `kernel/src/console/render.rs` con:
- `flush(grid, cache, surf)`: itera le righe dirty della griglia, compone ogni
  cella dirty nel back-buffer della Surface (maschera alpha × fg over bg tramite
  blend lineare per canale), blitta gli span dirty su MMIO, poi chiama
  `grid.clear_dirty()`.
- `compose_cell(...)`: funzione interna che copia la maschera alpha in un `Vec`
  locale prima di scrivere i pixel (evita conflitto di borrow `&GlyphMask` +
  `&mut Surface` simultanei).
- `blend(fg, bg, intensity)`: blend lineare per canale, coerente con `fb.rs`.

Aggiunto `pub mod render;` in `kernel/src/console/mod.rs`.

Aggiunte asserzioni T20–T22 in `kernel/src/console/engine_test.rs`:
- T20: un pixel con alpha 255 nella maschera di 'X' diventa il colore fg.
- T21: tale pixel esiste (la maschera non è vuota).
- T22: dopo flush la griglia non ha span dirty.

`make run-console-test` → `CONSOLE_TEST_PASS`.

## Perché
Task 7 del piano terminal-engine: il ponte griglia→pixel era mancante; senza
`render::flush` nessuna cella raggiunge il framebuffer.

## File toccati
- kernel/src/console/render.rs (nuovo)
- kernel/src/console/mod.rs
- kernel/src/console/engine_test.rs
- CHANGELOG/232-26-06-03-render-flush.md
