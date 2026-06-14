# 530 — Fix lag menu: damage raster su patch atlante (white-texel separation + pre-warm)

**Data:** 2026-06-14

## Cosa

`plan_damage` non forza più full-screen su un patch dell'atlante. Classifica ogni
prim come **white-only** (`white_only`: ogni vertice campiona `WHITE_UV=(0,0)` → fill
solido/wallpaper, indipendente dal CONTENUTO dell'atlante) e su `tex_dirty` danneggia
**solo** i prim atlas-dependent (testo). Il wallpaper full-screen è risparmiato.

- Core (mirror bit-identico): `ruos-raster/src/lib.rs` + `gui-core/src/raster.rs` —
  campo `PrimMeta.white_only`, fold in `prim_meta`, branch `tex_dirty`/`tex_changed`
  in `plan_damage`, `const WHITE_UV`. I **pixel non cambiano** (solo il rettangolo di
  danno) → cross-check byte-identico verde.
- Test: riscritto `texture_patch_forces_full_damage` →
  `texture_patch_damages_only_atlas_dependent_prims` (oracolo no-stale: incrementale
  == full render); nuovo cross-check `tex_patch_damage_matches_gui_core` (rect di
  danno **e** byte canvas identici gui-core vs wire); test mirror gui-core.
- Pre-warm (mitigazione app-side, rischio zero): `ruos-window` pre-alloca i glifi
  ASCII alle taglie non pre-caricate da egui (`proportional(14.0)` titlebar,
  `monospace(10.0)`) dopo il primo `ctx.run` (one-shot per-istanza) → i patch atlante
  cadono nel warmup, non durante l'interazione.

Test: ruos-raster 14 unit + 2 crosscheck VERDI; gui-core 45 VERDI.

## Perché

Root cause (changelog 528, misure HW): patch atlante font egui → `tex_dirty` →
`plan_damage` full-screen → raster ~300ms → loop a 8 it/s = il lag menu. Verificato
sul sorgente epaint 0.31.1: il texel bianco (0,0) è riservato e mai ri-scritto → un
prim white-only non diventa mai stale su un patch → risparmiabile. La rete di
sicurezza (re-raster su patch) resta per i prim atlas-dependent → nessun pixel stale.
Fix generale (qualsiasi finestra con sfondo a fill solido), bit-identità preservata.

Spec/piano: `docs/superpowers/specs/2026-06-14-raster-atlas-damage-design.md`,
`docs/superpowers/plans/2026-06-14-raster-atlas-damage.md`. Scope futuro documentato:
per-tex-id (wallpaper-immagine), SP2 (raster per-pixel overdraw), SP3 (present clip).
Nota: il warm dei titoli catalog nella shell è stato OMESSO (YAGNI: i titoli ASCII
sono già pre-caricati da egui; un titolo non-ASCII costa comunque poco col fix core).

## File toccati

- ruos-raster/src/lib.rs
- ruos-raster/src/tests.rs
- ruos-raster/tests/crosscheck.rs
- ruos-desktop/crates/gui-core/src/raster.rs
- ruos-desktop/crates/ruos-window/src/lib.rs
