//! `no_std` software rasterizer — 1:1 PORT of `gui-core::raster` operating on a
//! plain wire format (no egui, no tiny-skia). Produces BIT-IDENTICAL pixels to
//! `gui_core::raster::Renderer` for the same scene.
//!
//! Pipeline per pixel (tutto in sRGBA premoltiplicato, come `Color32` egui):
//!   frag = vertex_color ⊗ texel        (moltiplicazione per-canale)
//!   out  = frag + dst·(1 − frag.a)     (OVER premoltiplicato)
//!
//! The math here is copied VERBATIM from gui-core's `raster.rs`; only the type
//! plumbing (egui `Vertex`/`ClippedPrimitive`/`Color32`, tiny-skia `Pixmap`/
//! `PremultipliedColorU8`) is swapped for the wire structs below. Any pixel
//! divergence from gui-core is a regression; the cross-check test guards it.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// no_std float ops. `core` does NOT provide `f32/f64::floor/ceil/round` (they
// are std-only). To keep the ported math VERBATIM (`x.floor()`, etc.) while
// staying dependency-free, we provide them as exact IEEE-754 bit operations via
// an extension trait. floor/ceil/round are EXACT (not approximations like
// sin/exp): they round to an integer with no error, so these implementations
// produce results bit-identical to std's intrinsics — which is what makes the
// cross-check against gui-core (std) byte-for-byte equal.
//
// Under `cargo test` (cfg(test)), std is linked and provides inherent methods
// with the same names; the inherent methods win over trait methods, so tests
// transparently use std's versions — which are themselves IEEE-exact for these
// three functions, hence identical. In the no_std kernel build the trait
// methods are used.
trait F32Ext {
    fn floor(self) -> f32;
    fn ceil(self) -> f32;
    fn round(self) -> f32;
}

#[allow(dead_code)]
impl F32Ext for f32 {
    #[inline]
    fn floor(self) -> f32 {
        let t = trunc_f32(self);
        if self < 0.0 && t != self {
            t - 1.0
        } else {
            t
        }
    }
    #[inline]
    fn ceil(self) -> f32 {
        let t = trunc_f32(self);
        if self > 0.0 && t != self {
            t + 1.0
        } else {
            t
        }
    }
    #[inline]
    fn round(self) -> f32 {
        // Round half away from zero (matches std::f32::round).
        let t = trunc_f32(self);
        let frac = self - t;
        if self >= 0.0 {
            if frac >= 0.5 {
                t + 1.0
            } else {
                t
            }
        } else if frac <= -0.5 {
            t - 1.0
        } else {
            t
        }
    }
}

/// Truncate toward zero (exact). Bit-twiddling implementation; valid for all
/// finite/inf/NaN inputs.
#[inline]
fn trunc_f32(x: f32) -> f32 {
    let bits = x.to_bits();
    let exp = ((bits >> 23) & 0xff) as i32 - 127;
    // exp < 0: |x| < 1 → trunc is signed zero.
    if exp < 0 {
        return f32::from_bits(bits & 0x8000_0000);
    }
    // exp >= 23: already integral (or inf/NaN) → return as-is.
    if exp >= 23 {
        return x;
    }
    // Clear the fractional mantissa bits below the binary point.
    let mask = (1u32 << (23 - exp)) - 1;
    if bits & mask == 0 {
        return x; // already integral
    }
    f32::from_bits(bits & !mask)
}

/// Wire vertex (mirror egui::epaint::Vertex): pos, uv, color (Color32 premultiplied,
/// bytes [r,g,b,a] packed little-endian into u32).
#[derive(Clone, Copy)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub u: f32,
    pub v: f32,
    pub color: u32,
}

/// Wire primitive (mirror egui ClippedPrimitive for the Mesh case): clip rect,
/// texture id, half-open index range into the indices array.
#[derive(Clone, Copy)]
pub struct Prim {
    pub clip: [f32; 4],
    pub tex_id: u64,
    pub idx0: u32,
    pub idx1: u32,
}

/// Texture atlas: RGBA8888 premultiplied, row-major.
pub struct Atlas {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

/// Regione modificata (dirty rect) in pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirtyRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Rettangolo intero in pixel, semiaperto: [x0,x1) × [y0,y1).
#[derive(Clone, Copy)]
struct IRect {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl IRect {
    fn empty() -> Self {
        IRect { x0: 0, y0: 0, x1: 0, y1: 0 }
    }
    fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }
    fn union(self, o: IRect) -> IRect {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return self;
        }
        IRect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
    fn clamp(self, w: i32, h: i32) -> IRect {
        IRect {
            x0: self.x0.clamp(0, w),
            y0: self.y0.clamp(0, h),
            x1: self.x1.clamp(0, w),
            y1: self.y1.clamp(0, h),
        }
    }
}

/// Hash + bounding box (clip-clamped, in pixel) di una primitiva — per il diff
/// dirty-rect tra frame consecutivi.
struct PrimMeta {
    hash: u64,
    bbox: IRect,
}

/// Renderer con stato: atlanti texture (set/free), un canvas persistente (il
/// frame precedente) e i metadati per-primitiva dell'ultimo frame presentato,
/// per ridisegnare solo la regione cambiata (dirty-rect).
pub struct Raster {
    textures: BTreeMap<u64, Atlas>,
    /// Canvas persistente RGBA premoltiplicato, `w*h*4` byte.
    canvas: Vec<u8>,
    cw: u32,
    ch: u32,
    /// Hash+bbox per-primitiva dell'ultimo frame presentato (per il diff).
    prev: Vec<PrimMeta>,
    /// Colore di clear (sRGBA premoltiplicato).
    clear: [u8; 4],
    /// Set by `set_texture`; forces full damage on the next `plan_damage` (an atlas
    /// patch — e.g. egui font-atlas growth — changes pixels WITHOUT changing
    /// geometry, so the per-primitive hash diff can't detect it). Mirrors gui-core's
    /// `tex_changed → full` branch.
    tex_dirty: bool,
}

impl Raster {
    pub fn new(clear: [u8; 4]) -> Self {
        Self {
            textures: BTreeMap::new(),
            canvas: Vec::new(),
            cw: 0,
            ch: 0,
            prev: Vec::new(),
            clear,
            tex_dirty: false,
        }
    }

    /// Set/replace/patch a texture atlas. `pos = None` → full atlas (create or
    /// replace); `pos = Some((ox, oy))` → patch a sub-region of the existing
    /// atlas. `px` is RGBA8888 premultiplied, row-major, `w*h*4` bytes.
    pub fn set_texture(&mut self, id: u64, pos: Option<(u32, u32)>, w: u32, h: u32, px: &[u8]) {
        match pos {
            // Update parziale di una sub-regione dell'atlante esistente.
            Some((ox, oy)) => {
                if let Some(atlas) = self.textures.get_mut(&id) {
                    let aw = atlas.w as usize;
                    let pw = w as usize;
                    let ph = h as usize;
                    for row in 0..ph {
                        for col in 0..pw {
                            let dst = ((oy as usize + row) * aw + (ox as usize + col)) * 4;
                            let src = (row * pw + col) * 4;
                            atlas.px[dst] = px[src];
                            atlas.px[dst + 1] = px[src + 1];
                            atlas.px[dst + 2] = px[src + 2];
                            atlas.px[dst + 3] = px[src + 3];
                        }
                    }
                }
            }
            // Atlante nuovo / sostituzione completa.
            None => {
                let aw = w.max(1);
                let ah = h.max(1);
                let mut buf = Vec::with_capacity((aw * ah * 4) as usize);
                buf.extend_from_slice(&px[..((w * h * 4) as usize)]);
                // If px was empty (w or h == 0) the buf may be short; pad to aw*ah*4.
                buf.resize((aw * ah * 4) as usize, 0);
                self.textures.insert(id, Atlas { w: aw, h: ah, px: buf });
            }
        }
        self.tex_dirty = true;
    }

    /// (Re)alloc the canvas, compute this frame's damage rect. Returns
    /// `Some((x0,y0,x1,y1))` to raster, or `None` if nothing changed.
    pub fn plan_damage(
        &mut self,
        verts: &[Vertex],
        idx: &[u32],
        prims: &[Prim],
        width: u32,
        height: u32,
    ) -> Option<(i32, i32, i32, i32)> {
        let w = width.max(1);
        let h = height.max(1);
        let iw = w as i32;
        let ih = h as i32;
        let realloc = self.cw != w || self.ch != h || self.canvas.is_empty();
        if realloc {
            self.cw = w;
            self.ch = h;
            self.canvas = alloc::vec![0u8; (w * h * 4) as usize];
        }
        let meta: Vec<PrimMeta> = prims
            .iter()
            .map(|p| prim_meta(p, verts, idx, iw, ih))
            .collect();
        let full = IRect { x0: 0, y0: 0, x1: iw, y1: ih };
        let mut damage = IRect::empty();
        if realloc || self.tex_dirty || self.prev.len() != meta.len() {
            damage = full;
        } else {
            for (a, b) in self.prev.iter().zip(meta.iter()) {
                if a.hash != b.hash {
                    damage = damage.union(a.bbox).union(b.bbox);
                }
            }
        }
        let is_full = damage.x0 == 0 && damage.y0 == 0 && damage.x1 == iw && damage.y1 == ih;
        if !damage.is_empty() && !is_full {
            damage = IRect {
                x0: damage.x0 - 1,
                y0: damage.y0 - 1,
                x1: damage.x1 + 1,
                y1: damage.y1 + 1,
            }
            .clamp(iw, ih);
        }
        self.prev = meta;
        self.tex_dirty = false;
        if damage.is_empty() {
            return None;
        }
        Some((damage.x0, damage.y0, damage.x1, damage.y1))
    }

    /// Rasterizza le primitive nel canvas persistente, aggiornando SOLO il
    /// damage rect. Ritorna `(&canvas, dirty)`; `dirty.w==0 || dirty.h==0`
    /// significa "niente cambiato, non presentare".
    pub fn render(
        &mut self,
        verts: &[Vertex],
        idx: &[u32],
        prims: &[Prim],
        width: u32,
        height: u32,
    ) -> (&[u8], DirtyRect) {
        let dmg = match self.plan_damage(verts, idx, prims, width, height) {
            None => return (&self.canvas, DirtyRect { x: 0, y: 0, w: 0, h: 0 }),
            Some(d) => d,
        };
        let clear = self.clear;
        {
            let width_px = self.cw as i32;
            let ih = self.ch as i32;
            let mut band = Band { px: &mut self.canvas, width: width_px, y0: 0, y1: ih };
            raster_band(&mut band, dmg, clear, verts, idx, prims, &self.textures);
        }
        (
            &self.canvas,
            DirtyRect {
                x: dmg.0 as u32,
                y: dmg.1 as u32,
                w: (dmg.2 - dmg.0) as u32,
                h: (dmg.3 - dmg.1) as u32,
            },
        )
    }

    pub fn canvas(&self) -> &[u8] {
        &self.canvas
    }
}

/// Mutable view of canvas rows `[y0, y1)` (RGBA premultiplied), row-major, `width`
/// px per row, `px.len() == (y1 - y0) * width * 4`. Indices are ABSOLUTE screen
/// coords (py in [y0,y1), px in [0,width)); the row offset is removed internally.
/// Two bands with disjoint row ranges never alias.
pub struct Band<'a> {
    pub px: &'a mut [u8],
    pub width: i32,
    pub y0: i32,
    pub y1: i32,
}

impl Band<'_> {
    #[inline]
    fn get(&self, x: i32, y: i32) -> [u8; 4] {
        let i = (((y - self.y0) * self.width + x) * 4) as usize;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }
    #[inline]
    fn put(&mut self, x: i32, y: i32, c: [u8; 4]) {
        let i = (((y - self.y0) * self.width + x) * 4) as usize;
        self.px[i] = c[0];
        self.px[i + 1] = c[1];
        self.px[i + 2] = c[2];
        self.px[i + 3] = c[3];
    }
}

/// Fill the sub-rect `(x0,y0,x1,y1)` (half-open, already screen-clamped) with `rgba`,
/// but ONLY the rows that fall inside the band `[band.y0, band.y1)`.
fn fill_rect(band: &mut Band, rect: (i32, i32, i32, i32), rgba: [u8; 4]) {
    let (x0, y0, x1, y1) = rect;
    let y0 = y0.max(band.y0);
    let y1 = y1.min(band.y1);
    for y in y0..y1 {
        for x in x0..x1 {
            band.put(x, y, rgba);
        }
    }
}

/// Hash (FNV-1a su clip + vertici + indici + texture id) e bounding box
/// (intersezione estensione-vertici ∩ clip, in pixel, clampata) di una
/// primitiva. Due frame con lo stesso hash rendono gli stessi pixel.
///
/// gui-core hashes a `ClippedPrimitive`'s whole mesh; here a `Prim` references a
/// sub-range of shared arrays — so we hash the vertices reachable via
/// `idx[idx0..idx1]` and those indices (plus clip + tex_id) to preserve the same
/// change-detection semantics.
fn prim_meta(p: &Prim, verts: &[Vertex], idx: &[u32], iw: i32, ih: i32) -> PrimMeta {
    #[inline]
    fn mix(hsh: &mut u64, v: u32) {
        *hsh ^= v as u64;
        *hsh = hsh.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut hsh: u64 = 0xcbf2_9ce4_8422_2325;
    let c = p.clip; // [min.x, min.y, max.x, max.y]
    mix(&mut hsh, c[0].to_bits());
    mix(&mut hsh, c[1].to_bits());
    mix(&mut hsh, c[2].to_bits());
    mix(&mut hsh, c[3].to_bits());

    let mut minx = f32::INFINITY;
    let mut miny = f32::INFINITY;
    let mut maxx = f32::NEG_INFINITY;
    let mut maxy = f32::NEG_INFINITY;

    let id = p.tex_id;
    mix(&mut hsh, id as u32);
    mix(&mut hsh, (id >> 32) as u32);

    let tri_idx = &idx[p.idx0 as usize..p.idx1 as usize];
    for &i in tri_idx {
        let v = &verts[i as usize];
        mix(&mut hsh, v.x.to_bits());
        mix(&mut hsh, v.y.to_bits());
        mix(&mut hsh, v.u.to_bits());
        mix(&mut hsh, v.v.to_bits());
        mix(&mut hsh, v.color);
        minx = minx.min(v.x);
        miny = miny.min(v.y);
        maxx = maxx.max(v.x);
        maxy = maxy.max(v.y);
    }
    for &i in tri_idx {
        mix(&mut hsh, i);
    }

    // bbox = estensione vertici ∩ clip, in pixel interi, clampata allo schermo.
    let bx0 = minx.max(c[0]).floor() as i32;
    let by0 = miny.max(c[1]).floor() as i32;
    let bx1 = maxx.min(c[2]).ceil() as i32;
    let by1 = maxy.min(c[3]).ceil() as i32;
    let bbox = if bx0 >= bx1 || by0 >= by1 {
        IRect::empty()
    } else {
        IRect { x0: bx0, y0: by0, x1: bx1, y1: by1 }.clamp(iw, ih)
    };
    PrimMeta { hash: hsh, bbox }
}

/// Edge function (doppia area con segno) del triangolo (a,b,c) nel punto p.
///
/// In **f64**: i glifi di testo sono triangoli piccoli a coordinate schermo
/// grandi → in f32 `(px-ax)*(by-ay)` soffre cancellazione catastrofica e i pixel
/// di copertura risultano sbagliati. f64 dà margine sufficiente. KEEP f64.
#[inline]
fn edge(ax: f64, ay: f64, bx: f64, by: f64, px: f64, py: f64) -> f64 {
    (px - ax) * (by - ay) - (py - ay) * (bx - ax)
}

/// Un'arista è top-left (regola di copertura per evitare doppio-disegno dei
/// pixel sul bordo condiviso tra triangoli). y-down, winding CCW (area>0).
#[inline]
fn is_top_left(dx: f32, dy: f32) -> bool {
    dy < 0.0 || (dy == 0.0 && dx < 0.0)
}

/// Campiona l'atlante in (u,v) normalizzati con filtro bilineare. Centri dei
/// texel a +0.5; clamp ai bordi. Restituisce (r,g,b,a) in [0,255] premoltiplicato.
#[inline]
fn sample_bilinear(tex_px: &[u8], tex_w: i32, tex_h: i32, u: f32, v: f32) -> (f32, f32, f32, f32) {
    let fx = u * tex_w as f32 - 0.5;
    let fy = v * tex_h as f32 - 0.5;
    let x0f = fx.floor();
    let y0f = fy.floor();
    let dx = fx - x0f;
    let dy = fy - y0f;
    let ix0 = (x0f as i32).clamp(0, tex_w - 1);
    let iy0 = (y0f as i32).clamp(0, tex_h - 1);
    let ix1 = (x0f as i32 + 1).clamp(0, tex_w - 1);
    let iy1 = (y0f as i32 + 1).clamp(0, tex_h - 1);
    let sample = |ix: i32, iy: i32| -> (f32, f32, f32, f32) {
        let p = ((iy * tex_w + ix) * 4) as usize;
        (
            tex_px[p] as f32,
            tex_px[p + 1] as f32,
            tex_px[p + 2] as f32,
            tex_px[p + 3] as f32,
        )
    };
    let c00 = sample(ix0, iy0);
    let c10 = sample(ix1, iy0);
    let c01 = sample(ix0, iy1);
    let c11 = sample(ix1, iy1);
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let mix = |a: (f32, f32, f32, f32), b: (f32, f32, f32, f32), t: f32| {
        (lerp(a.0, b.0, t), lerp(a.1, b.1, t), lerp(a.2, b.2, t), lerp(a.3, b.3, t))
    };
    let top = mix(c00, c10, dx);
    let bot = mix(c01, c11, dx);
    mix(top, bot, dy)
}

/// Pure per-band rasterizer. Draws `clear` then all `prims` clipped to
/// `(damage ∩ [band.y0, band.y1))` into `band`. No threads, no global state: two
/// bands with disjoint rows can run concurrently.
pub fn raster_band(
    band: &mut Band,
    damage: (i32, i32, i32, i32),
    clear: [u8; 4],
    verts: &[Vertex],
    idx: &[u32],
    prims: &[Prim],
    textures: &BTreeMap<u64, Atlas>,
) {
    fill_rect(band, damage, clear);
    for p in prims {
        let clip = p.clip; // [min.x, min.y, max.x, max.y]
        let cx0 = (clip[0].floor() as i32).max(damage.0);
        let cy0 = (clip[1].floor() as i32).max(damage.1);
        let cx1 = (clip[2].ceil() as i32).min(damage.2);
        let cy1 = (clip[3].ceil() as i32).min(damage.3);
        if cx0 >= cx1 || cy0 >= cy1 {
            continue;
        }
        let tex = textures.get(&p.tex_id);
        let tri_idx = &idx[p.idx0 as usize..p.idx1 as usize];
        for tri in tri_idx.chunks_exact(3) {
            let v0 = &verts[tri[0] as usize];
            let v1 = &verts[tri[1] as usize];
            let v2 = &verts[tri[2] as usize];
            raster_tri(band, (cx0, cy0, cx1, cy1), tex, v0, v1, v2);
        }
    }
}

/// Rasterizza un triangolo: baricentrico + regola top-left, interpola colore +
/// uv, campiona l'atlante (bilineare), moltiplica, OVER-blend nel target.
fn raster_tri(
    band: &mut Band,
    clip: (i32, i32, i32, i32),
    tex: Option<&Atlas>,
    a: &Vertex,
    b: &Vertex,
    c: &Vertex,
) {
    // Garantisce winding CCW (area>0): se CW, scambia b<->c.
    let area0 = edge(
        a.x as f64, a.y as f64, b.x as f64, b.y as f64, c.x as f64, c.y as f64,
    );
    if area0 == 0.0 {
        return; // degenere
    }
    let (v0, v1, v2) = if area0 > 0.0 { (a, b, c) } else { (a, c, b) };

    let (x0, y0) = (v0.x, v0.y);
    let (x1, y1) = (v1.x, v1.y);
    let (x2, y2) = (v2.x, v2.y);

    let area = edge(
        x0 as f64, y0 as f64, x1 as f64, y1 as f64, x2 as f64, y2 as f64,
    ); // > 0 garantito
    let inv_area = 1.0 / area;

    // Top-left per ciascuna arista opposta al vertice i.
    let tl0 = is_top_left(x2 - x1, y2 - y1);
    let tl1 = is_top_left(x0 - x2, y0 - y2);
    let tl2 = is_top_left(x1 - x0, y1 - y0);

    // Bounding box clippato.
    let (cx0, cy0, cx1, cy1) = clip;
    let minx = (x0.min(x1).min(x2).floor() as i32).max(cx0);
    let miny = (y0.min(y1).min(y2).floor() as i32).max(cy0).max(band.y0);
    let maxx = (x0.max(x1).max(x2).ceil() as i32).min(cx1);
    let maxy = (y0.max(y1).max(y2).ceil() as i32).min(cy1).min(band.y1);
    if minx >= maxx || miny >= maxy {
        return;
    }

    let (tex_w, tex_h, tex_px): (i32, i32, &[u8]) = match tex {
        Some(t) => (t.w as i32, t.h as i32, &t.px),
        None => (0, 0, &[]),
    };

    // Solid-fill fast path: egui usa WHITE_UV (uv identici sui 3 vertici) per
    // tutto ciò che non è testo. Allora il texel campionato è costante sul
    // triangolo → campiona UNA volta e salta il bilinear per-pixel.
    let uv_const = v0.u == v1.u && v0.u == v2.u && v0.v == v1.v && v0.v == v2.v;
    let const_texel = if tex_w > 0 && uv_const {
        Some(sample_bilinear(tex_px, tex_w, tex_h, v0.u, v0.v))
    } else {
        None
    };

    for py in miny..maxy {
        let pyf = py as f32 + 0.5;
        for px in minx..maxx {
            let pxf = px as f32 + 0.5;

            // Edge function grezze (area>0 → inside = >=0); regola top-left sul bordo.
            let pxd = pxf as f64;
            let pyd = pyf as f64;
            let e0 = edge(x1 as f64, y1 as f64, x2 as f64, y2 as f64, pxd, pyd);
            let e1 = edge(x2 as f64, y2 as f64, x0 as f64, y0 as f64, pxd, pyd);
            let e2 = edge(x0 as f64, y0 as f64, x1 as f64, y1 as f64, pxd, pyd);
            let in0 = e0 > 0.0 || (e0 == 0.0 && tl0);
            let in1 = e1 > 0.0 || (e1 == 0.0 && tl1);
            let in2 = e2 > 0.0 || (e2 == 0.0 && tl2);
            if !(in0 && in1 && in2) {
                continue; // fuori, o bordo non-coperto da questo triangolo
            }
            let w0 = (e0 * inv_area) as f32;
            let w1 = (e1 * inv_area) as f32;
            let w2 = (e2 * inv_area) as f32;

            // Wire color: bytes [r,g,b,a] = color.to_le_bytes().
            let c0 = v0.color.to_le_bytes();
            let c1 = v1.color.to_le_bytes();
            let c2 = v2.color.to_le_bytes();

            // Colore per-vertice interpolato (premoltiplicato).
            let cr = w0 * c0[0] as f32 + w1 * c1[0] as f32 + w2 * c2[0] as f32;
            let cg = w0 * c0[1] as f32 + w1 * c1[1] as f32 + w2 * c2[1] as f32;
            let cb = w0 * c0[2] as f32 + w1 * c1[2] as f32 + w2 * c2[2] as f32;
            let ca = w0 * c0[3] as f32 + w1 * c1[3] as f32 + w2 * c2[3] as f32;

            // Texel: bianco opaco senza texture; costante per i fill; bilineare
            // per-pixel per il testo.
            let (tr, tg, tb, ta) = if tex_w == 0 {
                (255.0f32, 255.0, 255.0, 255.0)
            } else if let Some(ct) = const_texel {
                ct
            } else {
                let u = w0 * v0.u + w1 * v1.u + w2 * v2.u;
                let v = w0 * v0.v + w1 * v1.v + w2 * v2.v;
                sample_bilinear(tex_px, tex_w, tex_h, u, v)
            };

            // frag = vertex ⊗ texel  (entrambi premoltiplicati, normalizza /255).
            let fr = cr * tr / 255.0;
            let fg = cg * tg / 255.0;
            let fb = cb * tb / 255.0;
            let fa = ca * ta / 255.0;
            if fa <= 0.0 {
                continue; // trasparente: niente da comporre
            }

            // OVER premoltiplicato sopra il dst.
            let dst = band.get(px, py);
            let inv = 1.0 - fa / 255.0;
            let or = (fr + dst[0] as f32 * inv).round();
            let og = (fg + dst[1] as f32 * inv).round();
            let ob = (fb + dst[2] as f32 * inv).round();
            let oa = (fa + dst[3] as f32 * inv).round();

            let oa_u = oa.clamp(0.0, 255.0) as u8;
            // Mantiene l'invariante premoltiplicato r,g,b <= a.
            let clampc = |cc: f32| cc.clamp(0.0, oa).min(255.0) as u8;
            let out_c = [clampc(or), clampc(og), clampc(ob), oa_u];
            band.put(px, py, out_c);
        }
    }
}

#[cfg(test)]
mod tests;
