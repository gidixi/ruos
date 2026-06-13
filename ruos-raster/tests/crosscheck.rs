//! Cross-check: render the SAME scene with `gui_core::raster::Renderer` (egui +
//! tiny-skia) and with `ruos_raster::Raster` (wire format), assert the output is
//! BYTE-IDENTICAL. This is the guardian: any pixel divergence is a regression.
//! DO NOT weaken the assertion.

use egui::epaint::{
    image::ImageData, textures::TexturesDelta, Color32, ImageDelta, Mesh, Primitive, Vertex,
};
use egui::{pos2, ClippedPrimitive, ColorImage, Rect, TextureId, TextureOptions};
use std::sync::Arc;

use ruos_raster::{Prim, Raster, Vertex as WVertex};

const CLEAR: [u8; 4] = [0x1e, 0x1e, 0x1e, 0xff];

/// Texture 1x1 bianca su TextureId::Managed(0) (texel solido per i fill) —
/// mirrors gui-core's `white_texel_delta`.
fn white_texel_delta() -> TexturesDelta {
    let img = ColorImage { size: [1, 1], pixels: vec![Color32::WHITE] };
    let delta = ImageDelta::full(ImageData::Color(Arc::new(img)), TextureOptions::NEAREST);
    TexturesDelta { set: vec![(TextureId::Managed(0), delta)], free: vec![] }
}

fn epaint_vertex(pos: egui::Pos2, uv: egui::Pos2, color: Color32) -> Vertex {
    Vertex { pos, uv, color }
}

/// Mesh: rettangolo pieno (2 triangoli) di `color`, uv sul texel bianco.
fn rect_mesh(x0: f32, y0: f32, x1: f32, y1: f32, color: Color32) -> Mesh {
    let mut m = Mesh::with_texture(TextureId::Managed(0));
    let uv = pos2(0.0, 0.0);
    m.vertices.push(epaint_vertex(pos2(x0, y0), uv, color));
    m.vertices.push(epaint_vertex(pos2(x1, y0), uv, color));
    m.vertices.push(epaint_vertex(pos2(x1, y1), uv, color));
    m.vertices.push(epaint_vertex(pos2(x0, y1), uv, color));
    m.indices = vec![0, 1, 2, 0, 2, 3];
    m
}

/// Full-screen wallpaper + several colored rects + a semi-transparent rect.
/// Mirrors gui-core's `rich_scene`.
fn rich_scene(w: f32, h: f32) -> Vec<ClippedPrimitive> {
    let clip = Rect::from_min_max(pos2(0.0, 0.0), pos2(w, h));
    let mut v = vec![ClippedPrimitive {
        clip_rect: clip,
        primitive: Primitive::Mesh(rect_mesh(0.0, 0.0, w, h, Color32::from_rgb(10, 20, 30))),
    }];
    for i in 0..6u32 {
        let x = 4.0 + i as f32 * 9.0;
        v.push(ClippedPrimitive {
            clip_rect: clip,
            primitive: Primitive::Mesh(rect_mesh(
                x,
                x,
                x + 7.0,
                x + 30.0,
                Color32::from_rgb(200, (30 + i * 20) as u8, 80),
            )),
        });
    }
    v.push(ClippedPrimitive {
        clip_rect: clip,
        primitive: Primitive::Mesh(rect_mesh(
            8.0,
            10.0,
            w - 8.0,
            50.0,
            Color32::from_rgba_premultiplied(60, 30, 90, 128),
        )),
    });
    v
}

/// TextureId → wire tex_id (Managed(i) → i, User(i) → i | high bit).
fn tex_id_to_wire(id: TextureId) -> u64 {
    match id {
        TextureId::Managed(i) => i,
        TextureId::User(i) => i | 0x8000_0000_0000_0000,
    }
}

/// Flatten egui ClippedPrimitives (Mesh case) into shared wire arrays.
fn to_wire(prims: &[ClippedPrimitive]) -> (Vec<WVertex>, Vec<u32>, Vec<Prim>) {
    let mut verts: Vec<WVertex> = Vec::new();
    let mut idx: Vec<u32> = Vec::new();
    let mut wprims: Vec<Prim> = Vec::new();
    for cp in prims {
        let clip = [
            cp.clip_rect.min.x,
            cp.clip_rect.min.y,
            cp.clip_rect.max.x,
            cp.clip_rect.max.y,
        ];
        match &cp.primitive {
            Primitive::Mesh(m) => {
                let base = verts.len() as u32;
                for ev in &m.vertices {
                    verts.push(WVertex {
                        x: ev.pos.x,
                        y: ev.pos.y,
                        u: ev.uv.x,
                        v: ev.uv.y,
                        color: u32::from_le_bytes(ev.color.to_array()),
                    });
                }
                let i0 = idx.len() as u32;
                for &ix in &m.indices {
                    idx.push(base + ix);
                }
                let i1 = idx.len() as u32;
                wprims.push(Prim {
                    clip,
                    tex_id: tex_id_to_wire(m.texture_id),
                    idx0: i0,
                    idx1: i1,
                });
            }
            Primitive::Callback(_) => {}
        }
    }
    (verts, idx, wprims)
}

/// Convert the white-texel delta to a `set_texture` call on the wire Raster.
/// Color32 is already premultiplied; `to_array()` gives [r,g,b,a].
fn apply_delta(r: &mut Raster, deltas: &TexturesDelta) {
    for (id, delta) in &deltas.set {
        let (pw, ph, pixels): (usize, usize, Vec<Color32>) = match &delta.image {
            ImageData::Font(f) => {
                let [w, h] = f.size;
                (w, h, f.srgba_pixels(None).collect())
            }
            ImageData::Color(c) => {
                let [w, h] = c.size;
                (w, h, c.pixels.clone())
            }
        };
        let mut px: Vec<u8> = Vec::with_capacity(pw * ph * 4);
        for c in &pixels {
            let a = c.to_array();
            px.extend_from_slice(&a);
        }
        let wid = tex_id_to_wire(*id);
        let pos = delta.pos.map(|[ox, oy]| (ox as u32, oy as u32));
        r.set_texture(wid, pos, pw as u32, ph as u32, &px);
    }
}

#[test]
fn ruos_raster_matches_gui_core_bit_identical() {
    let (w, h) = (64u32, 64u32);
    let prims = rich_scene(w as f32, h as f32);
    let deltas = white_texel_delta();

    // Reference: gui-core (egui + tiny-skia).
    let mut r_a = gui_core::raster::Renderer::new();
    let (pixmap, dirty_a) = r_a.render(&prims, &deltas, w, h);
    let bytes_a = pixmap.data().to_vec();
    assert_eq!((dirty_a.w, dirty_a.h), (w, h), "gui-core first frame must be full");

    // Port: ruos-raster (wire format).
    let (verts, idx, wprims) = to_wire(&prims);
    let mut r_b = Raster::new(CLEAR);
    apply_delta(&mut r_b, &deltas);
    let (bytes_b, dirty_b) = r_b.render(&verts, &idx, &wprims, w, h);
    assert_eq!((dirty_b.w, dirty_b.h), (w, h), "ruos-raster first frame must be full");

    assert_eq!(bytes_a.len(), bytes_b.len(), "canvas length differs");
    if bytes_a.as_slice() != bytes_b {
        // Locate the first divergence for a useful failure message.
        for (i, (a, b)) in bytes_a.iter().zip(bytes_b.iter()).enumerate() {
            if a != b {
                let px = (i / 4) as u32;
                let (x, y) = (px % w, px / w);
                let ch = i % 4;
                panic!(
                    "ruos-raster diverges from gui-core at pixel ({x},{y}) channel {ch}: \
                     gui-core={a} ruos-raster={b}"
                );
            }
        }
    }
    assert_eq!(bytes_a.as_slice(), bytes_b, "ruos-raster diverges from gui-core");
}
