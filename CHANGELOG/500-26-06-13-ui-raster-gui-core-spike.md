# 500 — UI raster parallelo: gui-core band-able + spike PASS

**Data:** 2026-06-13

## Cosa
Primo milestone del piano 499 (raster UI parallelo), su branch `feat/ui-parallel-raster`:

- **gui-core band-able (submodule `ruos-desktop`, commit ee39f15 + 184a0d9):** refactor
  di `crates/gui-core/src/raster.rs` perché la rasterizzazione lavori per-banda di
  righe (`Band` view, `fill_rect`/`raster_tri` per-banda, `raster_band` puro,
  `plan_damage` estratta, `render` delega), output **bit-identico** al path
  whole-canvas. Nuovo test `banded_matches_serial_bit_identical` (seriale vs 3 bande
  disgiunte, byte-per-byte, con rect semi-trasparente che attraversa i bordi banda).
  44 test gui-core verdi. gui-core resta puro (niente thread). Due review (spec +
  qualità) passate.
- **SPIKE PASS:** validato che `std::thread::scope` fan-out+join DENTRO `frame()` di
  una finestra (mtwin) completa entro il deadline senza watchdog kill
  (`THREADS-WIN-OK` verde). Sonda rimossa da mtwin dopo la verifica. → il driver del
  Task 8 userà lo scope-join cooperativo (8a), non il fallback double-buffer.

## Perché
Fondamenta del rasterizzatore multi-core (stile llvmpipe): la decomposizione a bande
con garanzia bit-identica è il prerequisito sicuro per parallelizzare; lo spike
de-rischia il vincolo "niente blocking in frame()" prima di costruire il driver.

## File toccati
- ruos-desktop @ ee39f15..184a0d9 (puntatore submodule avanzato)
- docs/superpowers/plans/2026-06-13-ui-parallel-raster.md (esito spike)
- CHANGELOG/500-26-06-13-ui-raster-gui-core-spike.md (nuovo)
