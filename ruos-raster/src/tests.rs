//! Unit tests adapted from gui-core's `raster.rs` tests, built directly on the
//! wire `Vertex`/`Prim`/`idx` inputs (no egui here — the cross-check test in
//! `tests/crosscheck.rs` proves byte-identity with gui-core).

use super::*;

const CLEAR: [u8; 4] = [0x1e, 0x1e, 0x1e, 0xff];

/// A 1x1 white opaque texture on tex_id 0 (solid texel for fills) — mirrors
/// gui-core's `white_texel_delta`.
fn white_texel(r: &mut Raster) {
    r.set_texture(0, None, 1, 1, &[255, 255, 255, 255]);
}

/// Build a rect mesh (2 triangles) of `color`, uv on the white texel (0,0),
/// appended into shared `verts`/`idx`. Returns the half-open index range.
fn rect_mesh(
    verts: &mut Vec<Vertex>,
    idx: &mut Vec<u32>,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    color: u32,
) -> (u32, u32) {
    let base = verts.len() as u32;
    let (u, v) = (0.0, 0.0);
    verts.push(Vertex { x: x0, y: y0, u, v, color });
    verts.push(Vertex { x: x1, y: y0, u, v, color });
    verts.push(Vertex { x: x1, y: y1, u, v, color });
    verts.push(Vertex { x: x0, y: y1, u, v, color });
    let i0 = idx.len() as u32;
    idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    let i1 = idx.len() as u32;
    (i0, i1)
}

/// Pack RGBA bytes into a wire color (premultiplied), same as
/// `u32::from_le_bytes([r,g,b,a])`.
fn rgba(r: u8, g: u8, b: u8, a: u8) -> u32 {
    u32::from_le_bytes([r, g, b, a])
}

fn pixel(canvas: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [canvas[i], canvas[i + 1], canvas[i + 2], canvas[i + 3]]
}

#[test]
fn renders_solid_red_rect() {
    let mut r = Raster::new(CLEAR);
    white_texel(&mut r);
    let mut verts = Vec::new();
    let mut idx = Vec::new();
    let (i0, i1) = rect_mesh(&mut verts, &mut idx, 2.0, 2.0, 8.0, 8.0, rgba(255, 0, 0, 255));
    let prims = vec![Prim { clip: [0.0, 0.0, 10.0, 10.0], tex_id: 0, idx0: i0, idx1: i1 }];

    let (canvas, dirty) = r.render(&verts, &idx, &prims, 10, 10);
    assert_eq!(dirty, DirtyRect { x: 0, y: 0, w: 10, h: 10 });

    // Centro del rettangolo = rosso opaco.
    let c = pixel(canvas, 10, 5, 5);
    assert_eq!((c[0], c[1], c[2], c[3]), (255, 0, 0, 255));

    // Fuori dal rettangolo = clear (0x1e), non rosso.
    let bg = pixel(canvas, 10, 0, 0);
    assert_eq!((bg[0], bg[1], bg[2]), (0x1e, 0x1e, 0x1e));
}

#[test]
fn alpha_blends_over_background() {
    let mut r = Raster::new(CLEAR);
    white_texel(&mut r);
    // Rosso al 50% (premoltiplicato: rgb scalati con a).
    let half = rgba(128, 0, 0, 128);
    let mut verts = Vec::new();
    let mut idx = Vec::new();
    let (i0, i1) = rect_mesh(&mut verts, &mut idx, 0.0, 0.0, 4.0, 4.0, half);
    let prims = vec![Prim { clip: [0.0, 0.0, 4.0, 4.0], tex_id: 0, idx0: i0, idx1: i1 }];

    let (canvas, _) = r.render(&verts, &idx, &prims, 4, 4);
    let c = pixel(canvas, 4, 1, 1);
    // out.r = 128 + 0x1e*(1-128/255) ≈ 143
    assert!(c[0] > 135 && c[0] < 150, "red was {}", c[0]);
    assert!(c[1] < 20 && c[2] < 20);
}

/// Scena: rettangolo di sfondo statico + un piccolo rettangolo mobile.
fn scene(sx: f32, sy: f32, col: u32) -> (Vec<Vertex>, Vec<u32>, Vec<Prim>) {
    let clip = [0.0, 0.0, 64.0, 64.0];
    let mut verts = Vec::new();
    let mut idx = Vec::new();
    let (b0, b1) = rect_mesh(&mut verts, &mut idx, 0.0, 0.0, 64.0, 64.0, rgba(10, 20, 30, 255));
    let (r0, r1) = rect_mesh(&mut verts, &mut idx, sx, sy, sx + 8.0, sy + 8.0, col);
    let prims = vec![
        Prim { clip, tex_id: 0, idx0: b0, idx1: b1 },
        Prim { clip, tex_id: 0, idx0: r0, idx1: r1 },
    ];
    (verts, idx, prims)
}

/// Dirty-rect: spostare il rettangolo → l'update parziale del canvas deve
/// coincidere PIXEL-PER-PIXEL con un render full della nuova scena.
#[test]
fn dirty_rect_move_matches_full_render() {
    let (va, ia, pa) = scene(2.0, 2.0, rgba(255, 0, 0, 255));
    let (vb, ib, pb) = scene(40.0, 40.0, rgba(255, 0, 0, 255));

    let mut inc = Raster::new(CLEAR);
    white_texel(&mut inc);
    let _ = inc.render(&va, &ia, &pa, 64, 64); // primo frame: full
    let (inc_canvas, dirty) = inc.render(&vb, &ib, &pb, 64, 64); // parziale
    let inc_data = inc_canvas.to_vec();

    assert!(dirty.w > 0 && dirty.h > 0, "damage vuoto");
    assert!(dirty.w < 64 || dirty.h < 64, "atteso damage parziale, ho {dirty:?}");

    let mut full = Raster::new(CLEAR);
    white_texel(&mut full);
    let (full_canvas, _) = full.render(&vb, &ib, &pb, 64, 64);
    assert_eq!(
        inc_data.as_slice(),
        full_canvas,
        "canvas dirty-rect != render full (scia / regione stale)"
    );
}

/// Cambio di solo colore → update parziale corretto.
#[test]
fn dirty_rect_recolor_matches_full_render() {
    let (va, ia, pa) = scene(20.0, 20.0, rgba(255, 0, 0, 255));
    let (vb, ib, pb) = scene(20.0, 20.0, rgba(0, 255, 0, 255));

    let mut inc = Raster::new(CLEAR);
    white_texel(&mut inc);
    let _ = inc.render(&va, &ia, &pa, 64, 64);
    let (inc_canvas, _) = inc.render(&vb, &ib, &pb, 64, 64);
    let inc_data = inc_canvas.to_vec();

    let mut full = Raster::new(CLEAR);
    white_texel(&mut full);
    let (full_canvas, _) = full.render(&vb, &ib, &pb, 64, 64);
    assert_eq!(inc_data.as_slice(), full_canvas);
}

/// The no_std float ops (used in the kernel build) must be bit-identical to
/// std's intrinsics (used by gui-core). Under `cargo test`, std's inherent
/// methods shadow the trait in the ported code, so call the trait impls via
/// fully-qualified syntax and compare bit patterns against std over a wide range
/// of values. floor/ceil/round are EXACT for these inputs → must match exactly.
#[test]
fn float_ops_match_std_bit_identical() {
    use super::F32Ext;
    let mut vals: Vec<f32> = Vec::new();
    // Pixel-coordinate-ish values + fractions + negatives + edge cases.
    let mut x = -2000.0f32;
    while x <= 2000.0 {
        vals.push(x);
        x += 0.125;
    }
    for &k in &[0.0f32, -0.0, 0.5, -0.5, 0.49999997, 0.50000006, 1.5, -1.5, 2.5, -2.5] {
        vals.push(k);
    }
    // Larger magnitudes where the exponent path differs.
    for &k in &[8_388_608.0f32, 8_388_607.5, 16_777_216.0, -16_777_216.0, 123_456.78] {
        vals.push(k);
    }
    for v in vals {
        let mine_floor = F32Ext::floor(v).to_bits();
        let std_floor = f32::floor(v).to_bits();
        assert_eq!(mine_floor, std_floor, "floor({v}) mismatch");

        let mine_ceil = F32Ext::ceil(v).to_bits();
        let std_ceil = f32::ceil(v).to_bits();
        assert_eq!(mine_ceil, std_ceil, "ceil({v}) mismatch");

        let mine_round = F32Ext::round(v).to_bits();
        let std_round = f32::round(v).to_bits();
        assert_eq!(mine_round, std_round, "round({v}) mismatch");
    }
}

/// Scena invariata → damage vuoto (niente raster, niente blit).
#[test]
fn unchanged_scene_yields_empty_dirty() {
    let (v, i, p) = scene(10.0, 10.0, rgba(50, 60, 70, 255));
    let mut r = Raster::new(CLEAR);
    white_texel(&mut r);
    let (_, d1) = r.render(&v, &i, &p, 64, 64);
    assert_eq!((d1.w, d1.h), (64, 64)); // primo frame = full
    let (_, d2) = r.render(&v, &i, &p, 64, 64);
    assert_eq!((d2.w, d2.h), (0, 0)); // invariato → niente da presentare
}

/// Patch di una texture tra due frame a geometria IDENTICA → deve forzare full
/// damage: i pixel dell'atlante cambiano ma la geometria no, quindi il diff per
/// hash non può vederlo (l'hash non include i pixel dell'atlante). È il caso della
/// crescita incrementale dell'atlante font di egui. Senza `tex_dirty` il secondo
/// frame avrebbe damage vuoto → testo stale.
#[test]
fn texture_patch_forces_full_damage() {
    let (v, i, p) = scene(10.0, 10.0, rgba(50, 60, 70, 255));
    let mut r = Raster::new(CLEAR);
    white_texel(&mut r);
    let (_, d1) = r.render(&v, &i, &p, 64, 64);
    assert_eq!((d1.w, d1.h), (64, 64)); // primo frame = full
    let (_, d2) = r.render(&v, &i, &p, 64, 64);
    assert_eq!((d2.w, d2.h), (0, 0)); // baseline: invariato → vuoto
    // Patch dell'atlante (geometria identica).
    r.set_texture(0, Some((0, 0)), 1, 1, &[200, 200, 200, 255]);
    let (_, d3) = r.render(&v, &i, &p, 64, 64);
    assert_eq!((d3.w, d3.h), (64, 64), "un patch texture deve forzare full damage");
}

/// A richer multi-primitive scene (wallpaper + several colored rects + a
/// semi-transparent rect), built on the wire structs. Mirrors the spirit of
/// gui-core's `rich_scene` so the wire roundtrip exercises many prims.
fn rich_scene(w: f32, h: f32) -> (Vec<Vertex>, Vec<u32>, Vec<Prim>) {
    let clip = [0.0, 0.0, w, h];
    let mut verts = Vec::new();
    let mut idx = Vec::new();
    let mut prims = Vec::new();
    let (b0, b1) = rect_mesh(&mut verts, &mut idx, 0.0, 0.0, w, h, rgba(10, 20, 30, 255));
    prims.push(Prim { clip, tex_id: 0, idx0: b0, idx1: b1 });
    for i in 0..6u32 {
        let x = 4.0 + i as f32 * 9.0;
        let (r0, r1) = rect_mesh(
            &mut verts,
            &mut idx,
            x,
            x,
            x + 7.0,
            x + 30.0,
            rgba(200, (30 + i * 20) as u8, 80, 255),
        );
        prims.push(Prim { clip, tex_id: 0, idx0: r0, idx1: r1 });
    }
    let (s0, s1) = rect_mesh(&mut verts, &mut idx, 8.0, 10.0, w - 8.0, 50.0, rgba(60, 30, 90, 128));
    prims.push(Prim { clip, tex_id: 0, idx0: s0, idx1: s1 });
    (verts, idx, prims)
}

/// encode∘decode == identity for the raster: encode the typed mesh to the wire
/// format, `render_wire` it, and assert the canvas is byte-identical to
/// `render`ing the typed mesh directly. Proves the codec is lossless end-to-end.
#[test]
fn wire_roundtrip_matches_typed_render() {
    for (verts, idx, prims) in [
        scene(20.0, 20.0, rgba(255, 0, 0, 255)),
        rich_scene(64.0, 64.0),
    ] {
        // Reference: typed render.
        let mut typed = Raster::new(CLEAR);
        white_texel(&mut typed);
        let (typed_canvas, typed_dirty) = typed.render(&verts, &idx, &prims, 64, 64);
        let typed_data = typed_canvas.to_vec();

        // Port: encode to wire bytes, then render_wire.
        let vb = encode_verts(&verts);
        let ib = encode_indices(&idx);
        let pb = encode_prims(&prims);
        assert_eq!(vb.len(), verts.len() * VERTEX_WIRE_SIZE);
        assert_eq!(ib.len(), idx.len() * INDEX_WIRE_SIZE);
        assert_eq!(pb.len(), prims.len() * PRIM_WIRE_SIZE);

        let mut wire = Raster::new(CLEAR);
        white_texel(&mut wire);
        let (wire_canvas, wire_dirty) = wire.render_wire(&vb, &ib, &pb, 64, 64);

        assert_eq!(wire_dirty, typed_dirty, "wire dirty != typed dirty");
        assert_eq!(
            wire_canvas, typed_data.as_slice(),
            "render_wire canvas != render(typed) canvas — codec is lossy"
        );
    }
}

/// Decoders are panic-free on a roundtrip of well-formed data too.
#[test]
fn codec_roundtrip_is_identity() {
    let (verts, idx, prims) = rich_scene(64.0, 64.0);
    let dv = decode_verts(&encode_verts(&verts));
    let di = decode_indices(&encode_indices(&idx));
    let dp = decode_prims(&encode_prims(&prims));
    assert_eq!(dv.len(), verts.len());
    assert_eq!(di.len(), idx.len());
    assert_eq!(dp.len(), prims.len());
    for (a, b) in verts.iter().zip(dv.iter()) {
        assert_eq!((a.x, a.y, a.u, a.v, a.color), (b.x, b.y, b.u, b.v, b.color));
    }
    assert_eq!(idx, di);
    for (a, b) in prims.iter().zip(dp.iter()) {
        assert_eq!((a.clip, a.tex_id, a.idx0, a.idx1), (b.clip, b.tex_id, b.idx0, b.idx1));
    }
}

/// Trust boundary: decoding + rendering guest-controlled GARBAGE bytes must NEVER
/// panic. Truncated wire records are dropped; out-of-range idx0/idx1 and index
/// values referencing past the vertex array are guarded in the raster path.
#[test]
fn decode_is_panic_free_on_garbage() {
    // Truncated / nonsense buffers (not multiples of the record sizes).
    let mut r = Raster::new(CLEAR);
    white_texel(&mut r);
    let (canvas, _dirty) = r.render_wire(&[1, 2, 3], &[9, 9, 9, 9, 9], &[0xff; 10], 16, 16);
    assert_eq!(canvas.len(), 16 * 16 * 4, "should still produce a canvas");

    // Well-formed records, but a prim with idx0/idx1 out of range AND indices that
    // point past the (empty) vertex array → must be skipped, not panic.
    let verts: Vec<Vertex> = Vec::new(); // no vertices at all
    let idx: Vec<u32> = vec![0, 1, 2, 99, 100, 101];
    let prims = vec![
        // idx range entirely past idx.len()
        Prim { clip: [0.0, 0.0, 16.0, 16.0], tex_id: 0, idx0: 100, idx1: 200 },
        // idx range valid but indices reference missing vertices
        Prim { clip: [0.0, 0.0, 16.0, 16.0], tex_id: 0, idx0: 0, idx1: 6 },
        // inverted range (idx0 > idx1)
        Prim { clip: [0.0, 0.0, 16.0, 16.0], tex_id: 0, idx0: 5, idx1: 2 },
    ];
    let mut r2 = Raster::new(CLEAR);
    white_texel(&mut r2);
    let (vb, ib, pb) = (encode_verts(&verts), encode_indices(&idx), encode_prims(&prims));
    let (canvas2, _) = r2.render_wire(&vb, &ib, &pb, 16, 16);
    // Nothing was drawable → everything stays clear (no panic, no stale read).
    assert_eq!(pixel(canvas2, 16, 8, 8), CLEAR);
}

/// Trust boundary: malformed texture input (guest-controlled dims) must NEVER
/// panic — it is dropped. Mirrors what the kernel host fn would forward.
#[test]
fn malformed_texture_does_not_panic() {
    let mut r = Raster::new(CLEAR);
    // Full atlas but px too short for the declared 4×4 → dropped.
    r.set_texture(1, None, 4, 4, &[0u8; 4]);
    assert!(r.textures.get(&1).is_none(), "short full atlas must be dropped");
    // Zero dims → dropped.
    r.set_texture(2, None, 0, 0, &[]);
    assert!(r.textures.get(&2).is_none());
    // Overflowing dims → dropped (no arithmetic panic).
    r.set_texture(3, None, u32::MAX, u32::MAX, &[1, 2, 3, 4]);
    assert!(r.textures.get(&3).is_none());
    // Valid 2×2, then an out-of-bounds patch → dropped, atlas unchanged.
    r.set_texture(4, None, 2, 2, &[9u8; 16]);
    let before = r.textures.get(&4).unwrap().px.clone();
    r.set_texture(4, Some((1, 1)), 4, 4, &[7u8; 64]); // doesn't fit 2×2
    assert_eq!(r.textures.get(&4).unwrap().px, before, "out-of-fit patch must be dropped");
}
