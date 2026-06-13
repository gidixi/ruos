# 502 — ruos-raster: port no_std del rasterizzatore + cross-check bit-identico

**Data:** 2026-06-13

## Cosa

Nuova crate standalone `ruos-raster/`: porting 1:1 del rasterizzatore software di
`gui-core::raster` in `no_std`, dependency-free, che opera su un **wire format**
(struct semplici: `Vertex`, `Prim`, `Atlas` — niente egui, niente tiny-skia). La
math (edge f64, regola top-left, baricentrico, fast-path `const_texel`, bilinear,
OVER premoltiplicato con clamp dell'invariante r,g,b ≤ a, hash FNV + bbox per il
diff dirty-rect, banding) è copiata VERBATIM dal riferimento; cambia solo il
plumbing dei tipi (accesso colore via `u32::to_le_bytes()`, uv via `u`/`v`,
texture via `&Atlas.px` RGBA, canvas `Vec<u8>`, atlanti in `BTreeMap<u64,Atlas>`).

API: `Raster::new(clear)`, `set_texture`, `plan_damage`, `render` → `(&[u8],
DirtyRect)`, `canvas()`, e `raster_band()` libera (pub) per uso multi-banda.

Float ops: `core` non espone `f32::floor/ceil/round` (sono std-only). Per restare
dependency-free e tenere la math verbatim, fornite via extension trait con
implementazioni IEEE-754 ESATTE (bit-twiddling su `trunc` + correzione segno) —
floor/ceil/round sono esatte (non approssimazioni), quindi bit-identiche a std.
Sotto `cargo test` std è linkato e i metodi inerenti hanno priorità; un test
dedicato (`float_ops_match_std_bit_identical`) verifica via sintassi qualificata
che il trait combaci bit-per-bit con std su ~32k valori.

Test: 6 unit test (adattati da gui-core: rosso pieno, alpha-blend, dirty-rect
move/recolor = full render, scena invariata = dirty vuoto, float-ops) + il
**cross-check** `tests/crosscheck.rs`: la stessa scena (wallpaper + 6 riquadri +
rettangolo semitrasparente, come `rich_scene` di gui-core) renderizzata con
`gui_core::raster::Renderer` (egui+tiny-skia) e con `ruos_raster::Raster` →
`assert_eq!(bytes_a, bytes_b)` BYTE-IDENTICO. Tutti PASS.

Nessuna modifica a kernel o ruos-desktop (wiring kernel = task successivo).

## Perché

Far girare il rasterizzatore nel kernel (`no_std`) al posto del raster in-wasm
(pivot kernel-side raster, changelog 498-501). Il cross-check è il guardiano:
qualsiasi divergenza di pixel da gui-core è una regressione.

## File toccati

- ruos-raster/Cargo.toml
- ruos-raster/Cargo.lock
- ruos-raster/.gitignore
- ruos-raster/src/lib.rs
- ruos-raster/src/tests.rs
- ruos-raster/tests/crosscheck.rs
