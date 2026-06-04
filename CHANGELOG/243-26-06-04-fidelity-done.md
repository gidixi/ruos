# 243 — terminal-engine fidelity: Plan 2 done

**Data:** 2026-06-04

## Cosa
Aggiunto T35 (assertion end-to-end SGR via path vte completo) in `engine_test.rs`
e due cleanup richiesti dalla review:

- **T35**: `FramebufferConsole::write_str` con sequenza `ESC[1;4;38;2;200;100;50mA ESC[0mB`
  verifica che bold+underline+truecolor passino correttamente dall'input vte fino alla
  griglia, e che il cursore si trovi in colonna 2 dopo due char scritti.
- **Cleanup boxdraw.rs**: rimosso `use alloc::vec::Vec;` inutilizzato (il codice usa
  solo `vec![]` via `use alloc::vec;`).
- **Cleanup font.rs**: rimossa la funzione `raster_for` (nessun chiamante; tutto passa
  per `raster_for_weight`). Il doc-comment della funzione sopravvissuta aggiornato di
  conseguenza.
- **Cleanup glyphcache.rs**: aggiornato il module-doc da `font::raster_for` a
  `font::raster_for_weight`.

## Cosa ora si renderizza correttamente
- **Truecolor** (`ESC[38;2;r;g;b m` / `ESC[48;2;r;g;b m`): colori fg/bg a 24-bit
  applicati via `apply_sgr` → `Grid::set_fg/set_bg` → composited in `render::flush`.
- **Bold**: peso Noto Bold reale (`FontWeight::Bold`) via `raster_for_weight`; il
  GlyphCache distingue `(char, bold=true)` da `(char, bold=false)`.
- **Dim**: fg schiarita al 50% prima del blend alpha.
- **Underline**: riga orizzontale di 1 px sul fondo della cella (riga gh-2) in colore fg.
- **Reverse**: fg/bg swappati prima del blend.
- **Box-drawing subset ratatui** (light/heavy/double: linee, angoli, tee, croce;
  U+2500–257F): generati proceduralmente in `boxdraw.rs`; la cache restituisce la
  maschera senza passare per noto-sans-mono-bitmap (che non include il blocco).

## Limiti documentati
- **Angoli arrotondati** (╭ ╮ ╰ ╯, U+256D-256F-2570): resi come angoli light netti
  (arco vero = follow-up minore, deferred).
- **Box chars fuori dal subset ratatui**: cadono su '?' come ogni altro glyph assente.
- **Bold ASCII non pre-warm**: il primo bold char alloca in cache (una sola volta per
  char); il path Regular è pre-riscaldato a init.

## Perché
Completamento del Plan 2 (terminal-engine fidelity): tutto il rendering TUI (rtop,
box-drawing ratatui, SSH colorato) è ora verificato da T1–T35 + dalla regression
suite (run-console-test, run-test, run-rtop-test, run-pipe-test, run-ssh-test,
run-ctrlc-test: tutti PASS).

## File toccati
- kernel/src/console/engine_test.rs  (aggiunto T35)
- kernel/src/console/boxdraw.rs      (rimosso unused Vec import)
- kernel/src/console/font.rs         (rimossa raster_for)
- kernel/src/console/glyphcache.rs   (stale doc → raster_for_weight)
