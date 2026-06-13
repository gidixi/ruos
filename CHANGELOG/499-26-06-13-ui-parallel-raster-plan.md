# 499 — Piano: rasterizzatore UI software parallelo

**Data:** 2026-06-13

## Cosa
Piano implementativo (spike-first, TDD sul refactor host) per lo spec 498. Fasi:
- **0** baseline `wm-fps` su HW reale (numeri da utente).
- **1 SPIKE** — `std::thread::scope` fan-out+join DENTRO `frame()` su `tools/mtwin`:
  decide se il join cooperativo è praticabile (no watchdog kill) o serve il fallback
  double-buffer.
- **2 leva #0** — `repaint_delay` egui (viewport ROOT) → `wm.stay_awake` in
  `frame_once`/`frame_once_bare` + binding extern + `docs/api/wm.md`.
- **3 gui-core (TDD host)** — refactor `raster.rs`: `Band` view, `fill_rect`/`raster_tri`
  per-banda, `raster_band` puro, `plan_damage` pubblica, `render` delega; test
  **equivalenza serial↔band bit-identico**. `gui-core` resta puro (niente thread).
- **4 ruos-window** — driver split-canvas (`split_at_mut` disgiunto) + raster a bande,
  prima seriale poi parallelo (`std::thread::scope` se spike PASS, double-buffer se
  FAIL) + euristica n_bands + misura HW reale.

## Perché
Trasformare la baseline (spec 498: raster single-thread per finestra = collo) in un
rasterizzatore multi-core stile llvmpipe, mantenendo correttezza bit-identica e la
Regola d'oro di `gui-core`. Lo spike de-risca il vincolo "niente blocking in frame()"
prima di investire nel driver.

## File toccati
- docs/superpowers/plans/2026-06-13-ui-parallel-raster.md (nuovo)
- CHANGELOG/499-26-06-13-ui-parallel-raster-plan.md (nuovo)
