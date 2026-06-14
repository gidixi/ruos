# 529 — Spec design: damage raster su patch atlante (fix lag menu)

**Data:** 2026-06-14

## Cosa

Spec di design per il fix robusto del lag menu (root cause localizzata in 528):
`docs/superpowers/specs/2026-06-14-raster-atlas-damage-design.md`.

Approccio scelto (SP1): **white-texel damage separation** nel core bit-identico
(`plan_damage` risparmia i prim white-only — wallpaper/fill — su patch atlante,
danneggia solo i prim atlas-dependent) + **pre-warm atlante** app-side. Decomposto in
SP1 (questo spec), SP2 (raster per-pixel overdraw, separata), SP3 (present
clip-su-damage, separata).

## Perché

Il lag menu è `tex_dirty` (patch atlante font egui) → `plan_damage` ritorna FULL
damage → raster full-screen ~300ms → loop a 8 it/s. Verificato sul sorgente epaint
0.31.1 che il texel bianco (0,0) è riservato/immutabile → i prim white-only non
diventano mai stale su un patch → si possono risparmiare. Fix generale (qualsiasi
finestra con sfondo a fill solido), bit-identità preservata (cambia il rettangolo di
danno, non i pixel).

Piano d'implementazione (TDD, task bite-sized con codice mirror esatto) in
`docs/superpowers/plans/2026-06-14-raster-atlas-damage.md`.

## File toccati

- docs/superpowers/specs/2026-06-14-raster-atlas-damage-design.md
- docs/superpowers/plans/2026-06-14-raster-atlas-damage.md
