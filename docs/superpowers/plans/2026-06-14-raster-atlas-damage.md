# Raster atlas-damage fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminare il re-raster full-screen su patch dell'atlante font (causa del lag menu): `plan_damage` risparmia i prim *white-only* (fill solidi/wallpaper) e danneggia solo i prim *atlas-dependent*.

**Architecture:** Modifica mirror nel core bit-identico `ruos-raster/src/lib.rs` ↔ `gui-core/src/raster.rs` (classificazione `white_only` per-prim + branch tex-dirty che unisce solo i bbox dei prim non-white-only) + nuovo cross-check del patch-damage + pre-warm atlante app-side in `ruos-window`/shell. I pixel (`raster_band`) NON cambiano → bit-identità preservata; cambia solo il rettangolo di danno.

**Tech Stack:** Rust `no_std` (`ruos-raster`), Rust std (`gui-core` + tiny-skia), egui/epaint 0.31.1, build kernel via WSL.

**Riferimenti:** spec `docs/superpowers/specs/2026-06-14-raster-atlas-damage-design.md`; root cause changelog 528.

> **NOTA COMMIT (regola progetto CLAUDE.md):** gli step "Commit" sono inclusi per convenzione del piano, ma **non committare/pushare senza approvazione esplicita dell'utente**. In esecuzione: o si batcha e si chiede conferma, o si salta il commit finché l'utente non lo chiede.

> **GOTCHA build/test:** cargo gira **SOLO in WSL** su questa macchina:
> `wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && export PATH=$HOME/.cargo/bin:$PATH && <cmd>'`
> Test core: `cd ruos-raster && cargo test`. Test mirror: `cd ruos-desktop && cargo test -p gui-core --release`.

---

## File Structure

- `ruos-raster/src/lib.rs` — core: `PrimMeta.white_only`, `prim_meta` fold, `plan_damage` branch, `const WHITE_UV`.
- `ruos-raster/src/tests.rs` — riscrittura test patch (wire side) + helper `uv_rect`.
- `ruos-raster/tests/crosscheck.rs` — nuovo cross-check patch-damage (egui vs wire).
- `ruos-desktop/crates/gui-core/src/raster.rs` — mirror `PrimMeta`/`prim_meta`/`plan_damage` + test mirror.
- `ruos-desktop/crates/ruos-window/src/lib.rs` — pre-warm glifi (livello 2).
- `ruos-desktop/apps/shell/src/lib.rs` + `gui-core/src/desktop/shell.rs` — warm titoli catalog.
- `kernel/src/wasm/wt/wm.rs` — nessuna modifica logica (damage più stretto fluisce invariato).
- `CHANGELOG/530-*.md` — entry.

---

## Task 1: ruos-raster — white_only + scoped tex damage (core)

**Files:**
- Modify: `ruos-raster/src/lib.rs` (PrimMeta ~319, prim_meta ~613-674, plan_damage ~432-461, new const near 315)
- Test: `ruos-raster/src/tests.rs` (rewrite `texture_patch_forces_full_damage` ~250-263, add helper)

- [ ] **Step 1: Replace the old test with the failing new test**

In `ruos-raster/src/tests.rs`, add helper `uv_rect` (after `rect_mesh`):

```rust
/// Rect mesh with EXPLICIT uv (for a "glyph" prim that samples real atlas content,
/// uv != WHITE_UV). Returns the half-open index range.
fn uv_rect(
    verts: &mut Vec<Vertex>, idx: &mut Vec<u32>,
    x0: f32, y0: f32, x1: f32, y1: f32, u: f32, v: f32, color: u32,
) -> (u32, u32) {
    let base = verts.len() as u32;
    verts.push(Vertex { x: x0, y: y0, u, v, color });
    verts.push(Vertex { x: x1, y: y0, u, v, color });
    verts.push(Vertex { x: x1, y: y1, u, v, color });
    verts.push(Vertex { x: x0, y: y1, u, v, color });
    let i0 = idx.len() as u32;
    idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    let i1 = idx.len() as u32;
    (i0, i1)
}
```

Then DELETE the whole `texture_patch_forces_full_damage` test (tests.rs ~245-263) and replace with:

```rust
/// Un patch dell'atlante font NON deve forzare full damage: i prim white-only
/// (fill/wallpaper, uv==(0,0)) campionano il texel bianco riservato (0,0), mai
/// patchato → mai stale → risparmiati. Solo i prim atlas-dependent (uv!=(0,0),
/// "glifi") sono danneggiati. Oracolo: canvas incrementale == full render.
#[test]
fn texture_patch_damages_only_atlas_dependent_prims() {
    let (w, h) = (64u32, 64u32);
    let white4 = [255u8; 4 * 4 * 4]; // 4x4 atlas, tutto bianco

    let mut verts = Vec::new();
    let mut idx = Vec::new();
    // Wallpaper full-screen white-only (uv 0,0) + un "glifo" che campiona texel (2,2).
    let (w0, w1) = rect_mesh(&mut verts, &mut idx, 0.0, 0.0, w as f32, h as f32, rgba(10, 20, 30, 255));
    let (g0, g1) = uv_rect(&mut verts, &mut idx, 40.0, 40.0, 50.0, 50.0, 0.625, 0.625, rgba(255, 255, 255, 255));
    let clip = [0.0, 0.0, w as f32, h as f32];
    let prims = vec![
        Prim { clip, tex_id: 0, idx0: w0, idx1: w1 },
        Prim { clip, tex_id: 0, idx0: g0, idx1: g1 },
    ];

    let mut r = Raster::new(CLEAR);
    r.set_texture(0, None, 4, 4, &white4);
    let (_, d1) = r.render(&verts, &idx, &prims, w, h);
    assert_eq!((d1.w, d1.h), (w, h), "primo frame = full");
    let (_, d2) = r.render(&verts, &idx, &prims, w, h);
    assert_eq!((d2.w, d2.h), (0, 0), "frame invariato → vuoto");

    // Patch del texel (2,2) che il glifo campiona: bianco → rosso.
    r.set_texture(0, Some((2, 2)), 1, 1, &[255, 0, 0, 255]);
    let (canvas3, d3) = r.render(&verts, &idx, &prims, w, h);
    let canvas3 = canvas3.to_vec();

    // Danno = bbox del glifo (piccolo), NON full-screen.
    assert!(d3.w < w && d3.h < h, "patch NON deve forzare full, got {}x{}", d3.w, d3.h);
    assert!(d3.w > 0 && d3.h > 0, "il glifo deve essere danneggiato");
    assert!(d3.x <= 40 && d3.y <= 40 && d3.x + d3.w >= 50 && d3.y + d3.h >= 50, "il danno deve coprire il glifo");

    // Oracolo no-stale: incrementale == full render della scena patchata.
    let mut r_full = Raster::new(CLEAR);
    r_full.set_texture(0, None, 4, 4, &white4);
    r_full.set_texture(0, Some((2, 2)), 1, 1, &[255, 0, 0, 255]);
    let (full_canvas, _) = r_full.render(&verts, &idx, &prims, w, h);
    assert_eq!(canvas3.as_slice(), full_canvas, "incrementale != full → pixel stale");
}
```

- [ ] **Step 2: Run, verify it FAILS**

Run: `wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem/ruos-raster && export PATH=$HOME/.cargo/bin:$PATH && cargo test texture_patch_damages_only_atlas_dependent_prims'`
Expected: FAIL — oggi `tex_dirty` forza full, quindi `d3.w < w` fallisce (d3 == 64x64).

- [ ] **Step 3: Add `WHITE_UV` const**

In `ruos-raster/src/lib.rs`, after the `PrimMeta` struct (~322), add:

```rust
/// egui's reserved white-texel UV (epaint::WHITE_UV). Solid fills use it verbatim;
/// the texel at (0,0) is written once at atlas creation and NEVER patched, so a
/// prim sampling only (0,0) is atlas-CONTENT-independent (never stale on a patch).
const WHITE_UV: (f32, f32) = (0.0, 0.0);
```

- [ ] **Step 4: Add `white_only` to `PrimMeta`**

Replace the `PrimMeta` struct (lib.rs ~319-322):

```rust
struct PrimMeta {
    hash: u64,
    bbox: IRect,
    /// True se OGNI vertice referenziato campiona WHITE_UV (0,0): fill solido
    /// (wallpaper/pannelli) indipendente dal CONTENUTO dell'atlante → escluso dal
    /// danno su tex-dirty (un patch non lo rende mai stale).
    white_only: bool,
}
```

- [ ] **Step 5: Compute `white_only` in `prim_meta`**

In `prim_meta` (lib.rs ~613-674): before the vertex loop (right after `let mut maxy = f32::NEG_INFINITY;`, ~630) add:

```rust
    let mut white_only = true;
```

Inside the vertex loop, after `mix(&mut hsh, v.color);` (~653), add:

```rust
        white_only &= v.u == WHITE_UV.0 && v.v == WHITE_UV.1;
```

Change the return (lib.rs ~673) from `PrimMeta { hash: hsh, bbox }` to:

```rust
    PrimMeta { hash: hsh, bbox, white_only }
```

- [ ] **Step 6: Rework the `plan_damage` tex-dirty branch**

In `plan_damage` (lib.rs ~432-461), replace:

```rust
        let full = IRect { x0: 0, y0: 0, x1: iw, y1: ih };
        let mut damage = IRect::empty();
        if realloc || self.tex_dirty {
            damage = full;
        } else {
```

with:

```rust
        let full = IRect { x0: 0, y0: 0, x1: iw, y1: ih };
        let mut damage = IRect::empty();
        if realloc {
            damage = full;
        } else {
```

and, INSIDE that `else` block, AFTER the existing hash-diff `for` loops (after the `for m in &meta { if !old_hashes.contains(&m.hash) { damage = damage.union(m.bbox); } }` loop, ~460), add:

```rust
            // Patch dell'atlante (i pixel cambiano senza cambiare la geometria → l'hash
            // non lo vede): ridipingi SOLO i prim atlas-dependent. I white-only
            // campionano il texel (0,0) riservato (mai patchato) → mai stale → risparmiati.
            if self.tex_dirty {
                for m in &meta {
                    if !m.white_only {
                        damage = damage.union(m.bbox);
                    }
                }
            }
```

(Leave the `is_full`/dilation block, `self.prev = meta;`, `self.tex_dirty = false;` and the empty-check at the tail UNCHANGED.)

- [ ] **Step 7: Run the new test + full suite, verify PASS**

Run: `wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem/ruos-raster && export PATH=$HOME/.cargo/bin:$PATH && cargo test'`
Expected: PASS — `texture_patch_damages_only_atlas_dependent_prims` ok + all existing tests ok (`ruos_raster_matches_gui_core_bit_identical` still green: it's single-frame full render, unaffected).

- [ ] **Step 8: Commit** (solo se approvato — vedi nota commit)

```bash
git add ruos-raster/src/lib.rs ruos-raster/src/tests.rs
git commit -m "fix(raster): tex patch damages only atlas-dependent prims (spare white-only fills)"
```

---

## Task 2: gui-core — mirror white_only + scoped tex damage

**Files:**
- Modify: `ruos-desktop/crates/gui-core/src/raster.rs` (PrimMeta ~72, prim_meta ~330-392, plan_damage ~188-191)
- Test: `ruos-desktop/crates/gui-core/src/raster.rs` (test module, mirror test)

- [ ] **Step 1: Write the failing mirror test**

In `gui-core/src/raster.rs` test module (where the other raster tests live, ~691+), add. (Uses egui types already imported by the test module; if `Mesh`/`ClippedPrimitive`/`Primitive`/`TexturesDelta`/`ImageDelta`/`ColorImage`/`TextureOptions`/`Color32`/`pos2` aren't in scope there, import them at the top of the test fn from `egui`/`egui::epaint`.)

```rust
#[test]
fn tex_patch_damages_only_atlas_dependent_prims_gui() {
    use egui::epaint::{image::ImageData, textures::TexturesDelta, Color32, ImageDelta, Mesh, Primitive, Vertex};
    use egui::{pos2, ClippedPrimitive, ColorImage, Rect, TextureId, TextureOptions};
    use std::sync::Arc;

    let (w, h) = (64u32, 64u32);
    let clip = Rect::from_min_max(pos2(0.0, 0.0), pos2(w as f32, h as f32));
    let mk = |x0, y0, x1, y1, u, v, col: Color32| {
        let mut m = Mesh::with_texture(TextureId::Managed(0));
        m.vertices.push(Vertex { pos: pos2(x0, y0), uv: pos2(u, v), color: col });
        m.vertices.push(Vertex { pos: pos2(x1, y0), uv: pos2(u, v), color: col });
        m.vertices.push(Vertex { pos: pos2(x1, y1), uv: pos2(u, v), color: col });
        m.vertices.push(Vertex { pos: pos2(x0, y1), uv: pos2(u, v), color: col });
        m.indices = vec![0, 1, 2, 0, 2, 3];
        ClippedPrimitive { clip_rect: clip, primitive: Primitive::Mesh(m) }
    };
    let prims = vec![
        mk(0.0, 0.0, w as f32, h as f32, 0.0, 0.0, Color32::from_rgb(10, 20, 30)),       // wallpaper white-only
        mk(40.0, 40.0, 50.0, 50.0, 0.625, 0.625, Color32::WHITE),                         // "glyph"
    ];

    // 4x4 white atlas su Managed(0).
    let white4 = ColorImage { size: [4, 4], pixels: vec![Color32::WHITE; 16] };
    let d_full = TexturesDelta {
        set: vec![(TextureId::Managed(0), ImageDelta::full(ImageData::Color(Arc::new(white4)), TextureOptions::NEAREST))],
        free: vec![],
    };
    let empty = TexturesDelta::default();
    let patch = ColorImage { size: [1, 1], pixels: vec![Color32::from_rgb(255, 0, 0)] };
    let d_patch = TexturesDelta {
        set: vec![(TextureId::Managed(0), ImageDelta { image: ImageData::Color(Arc::new(patch)), pos: Some([2, 2]), options: TextureOptions::NEAREST })],
        free: vec![],
    };

    let mut r = Renderer::new();
    let (_, e1) = r.render(&prims, &d_full, w, h);
    assert_eq!((e1.w, e1.h), (w, h));
    let (_, e2) = r.render(&prims, &empty, w, h);
    assert_eq!((e2.w, e2.h), (0, 0));
    let (px3, e3) = r.render(&prims, &d_patch, w, h);
    let px3 = px3.data().to_vec();
    assert!(e3.w < w && e3.h < h, "patch must NOT force full, got {}x{}", e3.w, e3.h);
    assert!(e3.w > 0 && e3.h > 0);

    // Oracolo: full render della scena patchata.
    let mut rf = Renderer::new();
    let _ = rf.render(&prims, &d_full, w, h);
    let (pf, _) = rf.render(&prims, &d_patch, w, h);
    assert_eq!(px3.as_slice(), pf.data(), "incrementale != full (stale)");
}
```

- [ ] **Step 2: Run, verify it FAILS**

Run: `wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem/ruos-desktop && export PATH=$HOME/.cargo/bin:$PATH && cargo test -p gui-core --release tex_patch_damages_only_atlas_dependent_prims_gui'`
Expected: FAIL — oggi `tex_changed` forza full (`e3` == 64x64).

- [ ] **Step 3: Add `white_only` to `PrimMeta` (mirror)**

In `gui-core/src/raster.rs` replace `PrimMeta` (~72-75):

```rust
struct PrimMeta {
    hash: u64,
    bbox: IRect,
    /// Mirror di ruos-raster: true se ogni vertice campiona WHITE_UV (0,0) (fill
    /// solido atlas-independent) → escluso dal danno su tex change.
    white_only: bool,
}
```

- [ ] **Step 4: Compute `white_only` in `prim_meta` (mirror)**

In `prim_meta` (raster.rs ~330-392): before the `match &cp.primitive` (after `let mut maxy = f32::NEG_INFINITY;`, ~346) add:

```rust
    let mut white_only = true;
```

In the `Primitive::Mesh(m)` arm, inside the vertex loop after `mix(&mut hsh, u32::from_le_bytes(v.color.to_array()));` (~361) add:

```rust
                white_only &= v.uv.x == 0.0 && v.uv.y == 0.0;
```

(The `Primitive::Callback(_)` arm leaves `white_only = true` — callbacks aren't rastered, so excluding them from tex damage is correct.)

Change the return (raster.rs ~391) to:

```rust
    PrimMeta { hash: hsh, bbox, white_only }
```

- [ ] **Step 5: Rework the `plan_damage` tex branch (mirror)**

In `plan_damage` (raster.rs ~188-212), replace:

```rust
        if realloc || tex_changed {
            damage = full;
        } else {
```

with:

```rust
        if realloc {
            damage = full;
        } else {
```

and AFTER the existing hash-diff loops (after `for m in &meta { if !old_hashes.contains(&m.hash) { damage = damage.union(m.bbox); } }`, ~211) add inside the `else`:

```rust
            // Mirror ruos-raster: un cambio texture ridipinge solo i prim
            // atlas-dependent; i white-only (texel (0,0) riservato, mai patchato)
            // sono risparmiati → niente full-screen raster del wallpaper.
            if tex_changed {
                for m in &meta {
                    if !m.white_only {
                        damage = damage.union(m.bbox);
                    }
                }
            }
```

- [ ] **Step 6: Run mirror test + full gui-core suite, verify PASS**

Run: `wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem/ruos-desktop && export PATH=$HOME/.cargo/bin:$PATH && cargo test -p gui-core --release'`
Expected: PASS — new test ok + all 44 existing ok.

- [ ] **Step 7: Commit** (solo se approvato)

```bash
git add ruos-desktop/crates/gui-core/src/raster.rs
git commit -m "fix(gui-core): mirror tex-patch damage scoping (spare white-only fills)"
```

---

## Task 3: crosscheck — patch-damage byte-identity (ruos-raster vs gui-core)

**Files:**
- Test: `ruos-raster/tests/crosscheck.rs` (add helpers + new test; reuse `to_wire`/`apply_delta`/`tex_id_to_wire`)

- [ ] **Step 1: Add the new cross-check test**

Append to `ruos-raster/tests/crosscheck.rs`:

```rust
/// 4x4 white Color atlas su Managed(0) (full delta).
fn atlas4_white_delta() -> TexturesDelta {
    let img = ColorImage { size: [4, 4], pixels: vec![Color32::WHITE; 16] };
    let delta = ImageDelta::full(ImageData::Color(Arc::new(img)), TextureOptions::NEAREST);
    TexturesDelta { set: vec![(TextureId::Managed(0), delta)], free: vec![] }
}

/// Patch 1x1 a (2,2) col colore dato (partial delta, pos=Some).
fn patch_delta(ox: usize, oy: usize, c: Color32) -> TexturesDelta {
    let img = ColorImage { size: [1, 1], pixels: vec![c] };
    let delta = ImageDelta { image: ImageData::Color(Arc::new(img)), pos: Some([ox, oy]), options: TextureOptions::NEAREST };
    TexturesDelta { set: vec![(TextureId::Managed(0), delta)], free: vec![] }
}

/// Mesh "glifo": rect con uv esplicito (campiona contenuto reale dell'atlante).
fn glyph_mesh(x0: f32, y0: f32, x1: f32, y1: f32, u: f32, v: f32, color: Color32) -> Mesh {
    let mut m = Mesh::with_texture(TextureId::Managed(0));
    let uv = pos2(u, v);
    m.vertices.push(epaint_vertex(pos2(x0, y0), uv, color));
    m.vertices.push(epaint_vertex(pos2(x1, y0), uv, color));
    m.vertices.push(epaint_vertex(pos2(x1, y1), uv, color));
    m.vertices.push(epaint_vertex(pos2(x0, y1), uv, color));
    m.indices = vec![0, 1, 2, 0, 2, 3];
    m
}

/// Dopo un patch dell'atlante, gui-core e ruos-raster devono concordare su
/// rect di danno E byte del canvas (estende il guard oltre il single-frame).
#[test]
fn tex_patch_damage_matches_gui_core() {
    let (w, h) = (64u32, 64u32);
    let clip = Rect::from_min_max(pos2(0.0, 0.0), pos2(w as f32, h as f32));
    let prims = vec![
        ClippedPrimitive { clip_rect: clip, primitive: Primitive::Mesh(rect_mesh(0.0, 0.0, w as f32, h as f32, Color32::from_rgb(10, 20, 30))) }, // wallpaper white-only
        ClippedPrimitive { clip_rect: clip, primitive: Primitive::Mesh(glyph_mesh(40.0, 40.0, 50.0, 50.0, 0.625, 0.625, Color32::WHITE)) },       // glyph
    ];
    let d_full = atlas4_white_delta();
    let empty = TexturesDelta::default();
    let d_patch = patch_delta(2, 2, Color32::from_rgb(255, 0, 0));

    // gui-core
    let mut a = gui_core::raster::Renderer::new();
    let _ = a.render(&prims, &d_full, w, h);
    let _ = a.render(&prims, &empty, w, h);
    let (pa, da) = a.render(&prims, &d_patch, w, h);
    let bytes_a = pa.data().to_vec();

    // ruos-raster (wire)
    let (verts, idx, wprims) = to_wire(&prims);
    let mut b = Raster::new(CLEAR);
    apply_delta(&mut b, &d_full);
    let _ = b.render(&verts, &idx, &wprims, w, h);
    let _ = b.render(&verts, &idx, &wprims, w, h);
    apply_delta(&mut b, &d_patch);
    let (bytes_b, db) = b.render(&verts, &idx, &wprims, w, h);

    assert_eq!((da.x, da.y, da.w, da.h), (db.x, db.y, db.w, db.h), "damage rect differs");
    assert!(da.w < w && da.h < h, "patch must NOT be full-screen (got {}x{})", da.w, da.h);
    assert_eq!(bytes_a.as_slice(), bytes_b, "canvas diverges after patch");
}
```

- [ ] **Step 2: Run the cross-check, verify PASS**

Run: `wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem/ruos-raster && export PATH=$HOME/.cargo/bin:$PATH && cargo test --test crosscheck'`
Expected: PASS — both `ruos_raster_matches_gui_core_bit_identical` and `tex_patch_damage_matches_gui_core`. If damage rects differ → Task 1/2 logic diverged (fix the mismatch before proceeding).

- [ ] **Step 3: Commit** (solo se approvato)

```bash
git add ruos-raster/tests/crosscheck.rs
git commit -m "test(raster): cross-check tex-patch damage byte-identity gui-core vs wire"
```

---

## Task 4: Pre-warm font atlas (livello 2, app-side, rischio zero)

**Files:**
- Modify: `ruos-desktop/crates/ruos-window/src/lib.rs` (dopo il primo `ctx.run` in `frame_once`/`frame_once_bare`)
- Modify: `ruos-desktop/crates/gui-core/src/desktop/shell.rs` (warm titoli catalog)

> Egui pre-carica già l'ASCII delle taglie default (9.0/12.5/18.0 prop, 12.0 mono). Mancano: titlebar `proportional(14.0)`, System Monitor `monospace(10.0)`, e i titoli app dinamici. Il pre-warm collassa lo spike DENTRO il warmup, non durante l'hover. È mitigazione: col fix core (Task 1-2) anche uno spike sfuggito costa poco (danno = solo testo).

- [ ] **Step 1: Add a one-time pre-warm in ruos-window**

In `ruos-window/src/lib.rs`, add a helper (near the frame functions):

```rust
/// Forza l'allocazione dei glifi ASCII alle taglie usate dalla UI ma NON
/// pre-caricate da egui di default (titlebar 14.0, monospace 10.0). Eseguito una
/// volta dopo il primo ctx.run (quando ctx.fonts è valido) → i patch atlante
/// avvengono nel warmup, non durante l'interazione. Vedi spec atlas-damage.
fn prewarm_fonts(ctx: &egui::Context) {
    use egui::{FontId, FontFamily};
    let ascii: String = (32u8..=126).map(|b| b as char).collect();
    let ids = [
        FontId::new(14.0, FontFamily::Proportional),
        FontId::new(10.0, FontFamily::Monospace),
    ];
    ctx.fonts(|f| {
        for id in &ids {
            let _ = f.has_glyphs(&ascii, id);
        }
    });
}
```

> Verifica la firma reale di `has_glyphs` nella versione egui pinnata: in epaint 0.31 è `Fonts::has_glyphs(&self, s: &str, font_id: &FontId) -> bool`. Se l'ordine arg differisce, adatta. In alternativa equivalente: `let _ = f.glyph_width(id, c);` per ogni `c in ascii.chars()`.

- [ ] **Step 2: Call it once after the first ctx.run**

In BOTH `frame_once` and `frame_once_bare`, after `state.ctx.run(...)` returns, add (gate one-shot):

```rust
    {
        use core::sync::atomic::{AtomicBool, Ordering};
        static PREWARMED: AtomicBool = AtomicBool::new(false);
        if !PREWARMED.swap(true, Ordering::Relaxed) {
            prewarm_fonts(&state.ctx);
        }
    }
```

- [ ] **Step 3: Warm dynamic catalog titles in the shell**

In `gui-core/src/desktop/shell.rs`, where `shell_chrome` builds the catalog (the `app_list()`/catalog loop), after the catalog is obtained add:

```rust
    // Pre-warm dei glifi dei titoli app del catalog (dinamici, terzi) alla taglia
    // del launcher → un titolo nuovo viene "scaldato" al frame di refresh catalog,
    // non al primo hover. Vedi spec atlas-damage.
    ui.ctx().fonts(|f| {
        let id = egui::FontId::new(12.5, egui::FontFamily::Proportional);
        for e in &catalog {
            let _ = f.has_glyphs(&e.title, &id);
        }
    });
```

> Adatta `&catalog`/`e.title` ai nomi reali nel sorgente (la lista app e il campo titolo). Se `shell_chrome` non ha `ui.ctx()` a portata, usa il `ctx` disponibile nello scope.

- [ ] **Step 4: Build-check gui-core + ruos-window**

Run: `wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem/ruos-desktop && export PATH=$HOME/.cargo/bin:$PATH && cargo build -p gui-core -p ruos-window --release'`
Expected: compila pulito.

- [ ] **Step 5: Commit** (solo se approvato)

```bash
git add ruos-desktop/crates/ruos-window/src/lib.rs ruos-desktop/crates/gui-core/src/desktop/shell.rs
git commit -m "perf(desktop): pre-warm font atlas glyphs to avoid atlas patches during interaction"
```

---

## Task 5: Kernel build + cross-check verde + changelog + verifica HW

**Files:**
- Create: `CHANGELOG/530-26-06-14-raster-atlas-damage-fix.md`

- [ ] **Step 1: Full cross-check green (gate bit-identità)**

Run:
```
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem/ruos-raster && export PATH=$HOME/.cargo/bin:$PATH && cargo test'
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem/ruos-desktop && export PATH=$HOME/.cargo/bin:$PATH && cargo test -p gui-core --release'
```
Expected: tutti verdi (ruos-raster: unit + crosscheck incl. il nuovo; gui-core: 44 + il nuovo).

- [ ] **Step 2: Build measurement ISO**

Run: `wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && export PATH=$HOME/.cargo/bin:$PATH && make iso CARGO_FEATURES="wm-fps,netconsole"'`
Expected: `build/os.iso` prodotto, exit 0.

- [ ] **Step 3: Write changelog 530**

Create `CHANGELOG/530-26-06-14-raster-atlas-damage-fix.md`:

```markdown
# 530 — Fix lag menu: damage raster su patch atlante (white-texel separation + pre-warm)

**Data:** 2026-06-14

## Cosa
plan_damage non forza più full-screen su patch atlante: classifica i prim white-only
(uv==(0,0), fill/wallpaper) come atlas-independent e danneggia solo i prim
atlas-dependent (testo). Mirror ruos-raster + gui-core, nuovo cross-check patch-damage,
pre-warm glifi app-side (ruos-window + titoli catalog shell).

## Perché
Root cause (changelog 528): patch atlante font → tex_dirty → full-screen raster ~300ms
→ loop a 8 it/s = il lag menu. Il texel bianco (0,0) è riservato e mai patchato (egui
0.31.1) → i prim white-only non diventano mai stale → risparmiabili. Pixel invariati →
bit-identità preservata (cross-check verde).

## File toccati
- ruos-raster/src/lib.rs, ruos-raster/src/tests.rs, ruos-raster/tests/crosscheck.rs
- ruos-desktop/crates/gui-core/src/raster.rs
- ruos-desktop/crates/ruos-window/src/lib.rs
- ruos-desktop/crates/gui-core/src/desktop/shell.rs
```

- [ ] **Step 4: Commit** (solo se approvato)

```bash
git add CHANGELOG/530-26-06-14-raster-atlas-damage-fix.md
git commit -m "docs(changelog): 530 raster atlas-damage fix"
```

- [ ] **Step 5: Verifica HW reale (utente)**

Flasha `build/os.iso`, boota (ethernet), `tools/netconsole-rx`, muovi sul menu come per 528.
Expected: durante il movimento `wmfps3` mostra `area_max` piccolo (NON 2073600), `ra_max`
pochi ms (NON ~300ms), loop ≥60 it/s. Menu fluido. Se `area_max` resta full → il patch
arriva come `pos=None` (atlas re-create) e non `pos=Some`: vedi open question nello spec
(per-tex-id / pre-warm più aggressivo).

---

## Self-Review (eseguito dall'autore del piano)

- **Spec coverage:** white-texel core (Task 1+2), cross-check esteso (Task 3), pre-warm (Task 4), build+HW+changelog (Task 5). Per-tex-id / SP2 / SP3 esplicitamente fuori scope nello spec → nessun task (corretto).
- **Placeholder scan:** nessun TBD/TODO; codice completo in ogni step di codice. Le due note "adatta la firma/i nomi reali" (has_glyphs, catalog) sono verifiche puntuali su API esterne, non placeholder logici.
- **Type consistency:** `white_only` (campo bool) usato identico in entrambi i `PrimMeta`/`prim_meta`/`plan_damage`; `WHITE_UV` const in ruos-raster, literali `0.0` in gui-core (commentati come WHITE_UV). DirtyRect campi `x,y,w,h` usati come da sorgente. `render`/`set_texture`/`Renderer::render` firme come da sorgente letto.
```
