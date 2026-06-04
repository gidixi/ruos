# 241 — Bold font weight rendering

**Data:** 2026-06-04

## Cosa
`GlyphCache::mask('M', true)` ora ritorna una maschera diversa da quella Regular:
usa il vero peso **Bold** del font Noto Sans Mono (feature `bold` del crate
`noto-sans-mono-bitmap`), non una sintesi software.

- `kernel/Cargo.toml`: aggiunta feature `"bold"` alla dep `noto-sans-mono-bitmap`.
- `kernel/src/console/font.rs`: aggiunta `pub fn raster_for_weight(ch, bold)` che
  sceglie `FontWeight::Bold` o `FontWeight::Regular` e cade su '?' se il glifo
  manca, infine su '?' Regular.
- `kernel/src/console/glyphcache.rs`: `rasterize` ora prende `bold: bool` e chiama
  `raster_for_weight`; `mask` passa `bold` a `rasterize`. L'import di `raster_for`
  è stato rimosso (non più usato da glyphcache).
- `kernel/src/console/engine_test.rs`: aggiunta asserzione **T32** (bold ≠ regular
  per 'M').

## Perché
Task 4 del Piano 2 (terminal-engine fidelity): il testo in bold deve essere
renderizzato con il font weight corretto invece di usare sempre Regular.

Path scelto: **peso Bold reale** (non sintesi per dilation). Verificato che
`get_raster_width(FontWeight::Bold, Size24) == 11 ==
get_raster_width(FontWeight::Regular, Size24)`: la larghezza di cella rimane
invariata, il grid layout non è rotto.

## File toccati
- kernel/Cargo.toml
- kernel/Cargo.lock
- kernel/src/console/font.rs
- kernel/src/console/glyphcache.rs
- kernel/src/console/engine_test.rs
