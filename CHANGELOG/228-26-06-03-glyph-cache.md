# 228 — Alpha-mask glyph cache (Plan 1)

**Data:** 2026-06-03

## Cosa
Aggiunto `kernel/src/console/glyphcache.rs`: cache `GlyphCache` che mappa
`(char, bool)` → `GlyphMask` (buffer flat `w*h` di intensità 0..255, row-major).
Le maschere sono ricavate una volta da `font::raster_for` e poi riusate; il flag
`bold` è in chiave ma nel Plan 1 usa sempre il peso Regular.
Aggiunte asserzioni T4–T6 in `engine_test.rs` (dimensioni cella, pixel accesi su
'X', tutto trasparente su ' '). Dichiarato `pub mod glyphcache` in `mod.rs`.

## Perché
Task 3 del piano terminal-engine: separare il layer alpha-mask (char+bold) dal
layer colore per evitare esplosione di entry truecolor nella cache.

## File toccati
- kernel/src/console/glyphcache.rs (nuovo)
- kernel/src/console/mod.rs
- kernel/src/console/engine_test.rs
- CHANGELOG/228-26-06-03-glyph-cache.md
