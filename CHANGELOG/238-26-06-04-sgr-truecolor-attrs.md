# 238 — SGR truecolor + attribute parsing

**Data:** 2026-06-04

## Cosa
Esteso `apply_sgr` in `ansi.rs` per gestire:
- Truecolor fg/bg via `38;2;r;g;b` e `48;2;r;g;b`
- Attributi testo: 1=bold, 2=dim, 4=underline, 7=reverse; 22/24/27 per reset selettivo
- Firma aggiornata da `(params, fg, bg) -> (Rgb,Rgb)` a `(params, fg, bg, attr) -> (Rgb,Rgb,CellAttr)`

Aggiunto getter `current_attr()` su `Grid`. Aggiornato il handler `'m'` in `fb.rs` per
passare e ricevere `CellAttr`. Aggiunte asserzioni T25-T28 in `engine_test.rs`.

## Perché
Task 1 del Plan 2 (terminal-engine fidelity): parsing/storage di truecolor e attributi
come fondamenta per il rendering attributi nei task successivi. Il rendering non cambia
ancora (YAGNI); i colori fg/bg truecolor risultano già visibili perché `compose` usa
`cell.fg`/`cell.bg`.

## File toccati
- kernel/src/console/ansi.rs
- kernel/src/console/grid.rs
- kernel/src/console/fb.rs
- kernel/src/console/engine_test.rs
- CHANGELOG/238-26-06-04-sgr-truecolor-attrs.md
