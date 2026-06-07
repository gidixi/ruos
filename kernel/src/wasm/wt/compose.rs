//! Pure compositing kernel shared by the serial reference path and the
//! SMP-parallel band jobs. NO kernel globals, NO I/O, NO allocation — a band
//! job runs this on an AP, the 1-CPU fallback runs it on the BSP, and the
//! serial reference runs it on the BSP; identical inputs ⇒ identical output.
//!
//! The back-buffer is RGBX8888, row-major, `stride` BYTES per row (>= w*4).
//! Compositing is painter's algorithm: callers pass windows already sorted
//! back-to-front (`wins` Vec order = z-order); each window is opaque (alpha
//! ignored — the SP3 footprints are solid RGBA, last writer wins per pixel).
//!
//! NOTE: a `WinDesc` describes ONE source rect produced by
//! `Compositor::compose_window`. Under CSD (SP-B) that is the window's RAW
//! committed surface (the app draws its own title bar / [X]); the band kernel
//! paints a rect of RGBA pixels at a screen (x,y) regardless of whether the
//! source is a raw surface or a (legacy) decorated footprint, so this code is
//! unchanged — only the WinDesc CONSTRUCTION (in `wm.rs`) changed.

/// One decorated footprint as the band kernel sees it: a raw pointer to its
/// RGBA8888 footprint buffer plus its on-screen rect. `'static`-free; the
/// dispatcher guarantees the pixels outlive the job (BSP blocks on join before
/// freeing). NOTE: `px` points at a decorated footprint (`compose_window`), not
/// a raw window surface — decorations are part of the source pixels here.
#[derive(Copy, Clone)]
pub struct WinDesc {
    pub px: *const u8, // footprint base (RGBA8888, src_stride = w*4)
    pub px_len: usize, // footprint length in bytes (bounds guard)
    pub x: u32,        // on-screen top-left x
    pub y: u32,        // on-screen top-left y
    pub w: u32,        // footprint width  (px)
    pub h: u32,        // footprint height (px)
    pub shadow: bool,  // cast a drop shadow under this window (false for the bg)
}

/// Drop-shadow parameters (global v1; the same soft shadow under every non-bg
/// window). The shadow is the window rect OFFSET by `(SHADOW_DX, SHADOW_DY)` and
/// feathered `SHADOW_R` px outward; only the part outside the window's own opaque
/// rect is visible (the surface overwrites the interior). All-integer falloff so
/// the parallel band composite is bit-identical to the serial reference (no f32
/// divergence across cores — see the SP4 equivalence test).
const SHADOW_R: usize = 10;       // feather radius (px) — tight, not a wide halo
const SHADOW_DX: i32 = 4;         // shadow x offset (cast down-right, hugging the window)
const SHADOW_DY: i32 = 6;         // shadow y offset
const SHADOW_MAX: u32 = 70;       // peak shadow alpha (0..=255) — subtle, not heavy

/// Cubic falloff LUT: `LUT[d] = round(255 * (1 - d/R)^3)` for `d` in `0..=R`.
/// `LUT[0] = 255` (full, at the rect edge), `LUT[R] = 0`. The cube concentrates the
/// shadow near the edge (a defined contact shadow that fades fast) instead of the
/// wide, soft glow a quadratic/gaussian gives — reads more like a real cast shadow.
const SHADOW_LUT: [u8; SHADOW_R + 1] = {
    let mut t = [0u8; SHADOW_R + 1];
    let r3 = (SHADOW_R * SHADOW_R * SHADOW_R) as u32;
    let mut d = 0usize;
    while d <= SHADOW_R {
        let k = (SHADOW_R - d) as u32; // R - d
        t[d] = ((255 * k * k * k) / r3) as u8;
        d += 1;
    }
    t
};

/// Shadow alpha (0..=255) for a pixel `dx_out`/`dy_out` px OUTSIDE the (offset)
/// shadow rect along each axis (0 = inside that axis' span). Separable product of
/// the per-axis falloff, scaled by `SHADOW_MAX`. Returns 0 past the feather radius.
#[inline]
fn shadow_alpha(dx_out: usize, dy_out: usize) -> u32 {
    if dx_out > SHADOW_R || dy_out > SHADOW_R { return 0; }
    let fx = SHADOW_LUT[dx_out] as u32;
    let fy = SHADOW_LUT[dy_out] as u32;
    // SHADOW_MAX * fx/255 * fy/255, integer.
    (SHADOW_MAX * fx * fy) / (255 * 255)
}

// SAFETY: WinDesc only carries a raw pointer + plain integers. It is read-only
// in the band kernel and the dispatcher keeps the pointee alive across the
// join. We send descriptors to AP cores, so assert the marker traits.
unsafe impl Send for WinDesc {}
unsafe impl Sync for WinDesc {}

/// Composite `wins` (back-to-front) into rows `[band_y0, band_y1)` of a
/// screen-sized RGBX back-buffer.
///
/// `back` points at back-buffer row 0; `stride` is its byte pitch; the buffer
/// is `screen_w` px wide and at least `band_y1` rows tall. Every write lands in
/// `[band_y0, band_y1)` ⇒ two bands with disjoint ranges never alias, so two
/// jobs may run this concurrently on different cores with no synchronization.
///
/// SAFETY: caller guarantees `back .. back + band_y1*stride` is a valid,
/// uniquely-owned (for this band's rows) writable region, and each
/// `WinDesc.px .. px+px_len` is a valid readable footprint.
pub unsafe fn composite_band(
    back: *mut u8,
    stride: usize,
    screen_w: u32,
    band_y0: u32,
    band_y1: u32,
    bg: u32, // background RGBX (little-endian u32) for uncovered pixels
    wins: &[WinDesc],
) {
    let sw = screen_w as usize;
    // 1) Clear this band to the background colour.
    let mut row = band_y0 as usize;
    while row < band_y1 as usize {
        let row_ptr = back.add(row * stride) as *mut u32;
        let mut col = 0usize;
        while col < sw {
            // SAFETY: col < screen_w and row < band_y1 ⇒ within the band.
            *row_ptr.add(col) = bg;
            col += 1;
        }
        row += 1;
    }
    // 2) Painter's algorithm: blit each footprint's overlap with this band.
    for win in wins {
        // Drop shadow FIRST: blend it over everything already painted (bg + lower
        // windows), then the opaque surface below covers the shadow's interior.
        // Painter order (bottom->top) makes a window's shadow land on the windows
        // beneath it and never on the ones above (those paint later).
        if win.shadow {
            blend_shadow_band(back, stride, screen_w, band_y0, band_y1, win);
        }
        let wx = win.x as usize;
        let wy = win.y as usize;
        let ww = win.w as usize;
        let wh = win.h as usize;
        if ww == 0 || wh == 0 { continue; }
        let src_stride = ww * 4; // footprint is RGBA8888
        // Vertical overlap of the footprint with this band, clipped to screen rows.
        let win_y1 = wy + wh;
        let y_start = core::cmp::max(wy, band_y0 as usize);
        let y_end = core::cmp::min(win_y1, band_y1 as usize);
        if y_start >= y_end { continue; }
        // Horizontal clip to the screen width.
        if wx >= sw { continue; }
        let vis_w = core::cmp::min(ww, sw - wx);
        let mut sy = y_start;
        while sy < y_end {
            let src_row_off = (sy - wy) * src_stride;
            let src_end = src_row_off + vis_w * 4;
            if src_end > win.px_len { break; }
            let src = win.px.add(src_row_off);
            let dst = back.add(sy * stride + wx * 4);
            // RGBA src → RGBX dst: identical byte order (alpha lands in the
            // ignored X slot). One row memcpy. (BGR conversion, if any, is done
            // once at present time by gfx::blit — the back-buffer is canonical
            // RGBX, matching the SP3 footprint format.)
            core::ptr::copy_nonoverlapping(src, dst, vis_w * 4);
            sy += 1;
        }
    }
}

/// Blend one window's drop shadow into rows `[band_y0, band_y1)` of the RGBX
/// back-buffer. The shadow is the window rect offset by `(SHADOW_DX, SHADOW_DY)`,
/// feathered `SHADOW_R` px outward; pixels under the window's OWN opaque rect are
/// skipped (the surface copy overwrites them). Darken-only blend toward black:
/// `out = dst * (255 - alpha) / 255` per channel — all integer, so the parallel
/// band composite stays bit-identical to the serial reference.
///
/// SAFETY: same contract as `composite_band` — `back .. back + band_y1*stride` is
/// a valid, uniquely-owned (for this band's rows) writable region, and every write
/// lands in `[band_y0, band_y1)`.
unsafe fn blend_shadow_band(
    back: *mut u8,
    stride: usize,
    screen_w: u32,
    band_y0: u32,
    band_y1: u32,
    win: &WinDesc,
) {
    if win.w == 0 || win.h == 0 { return; }
    let sw = screen_w as i32;
    let r = SHADOW_R as i32;
    // Window's own opaque rect (un-offset) — skip; the surface covers it.
    let wx0 = win.x as i32;
    let wy0 = win.y as i32;
    let wx1 = wx0 + win.w as i32;
    let wy1 = wy0 + win.h as i32;
    // Offset shadow rect (the falloff is measured from this rect's edges).
    let ox0 = wx0 + SHADOW_DX;
    let oy0 = wy0 + SHADOW_DY;
    let ox1 = ox0 + win.w as i32;
    let oy1 = oy0 + win.h as i32;
    // Shadow bounding box = offset rect grown by R, clipped to screen + this band.
    let bx0 = core::cmp::max(ox0 - r, 0);
    let bx1 = core::cmp::min(ox1 + r, sw);
    let by0 = core::cmp::max(oy0 - r, band_y0 as i32);
    let by1 = core::cmp::min(oy1 + r, band_y1 as i32);
    if bx0 >= bx1 || by0 >= by1 { return; }

    let mut py = by0;
    while py < by1 {
        // Distance of this row outside the offset rect's y-span (0 = inside).
        let dy_out = if py < oy0 { (oy0 - py) as usize }
                     else if py >= oy1 { (py - oy1 + 1) as usize }
                     else { 0 };
        if dy_out > SHADOW_R { py += 1; continue; }
        let inside_win_y = py >= wy0 && py < wy1;
        let row = back.add(py as usize * stride);
        let mut px = bx0;
        while px < bx1 {
            // Skip the window's own opaque interior (overwritten by its surface).
            if inside_win_y && px >= wx0 && px < wx1 { px += 1; continue; }
            let dx_out = if px < ox0 { (ox0 - px) as usize }
                         else if px >= ox1 { (px - ox1 + 1) as usize }
                         else { 0 };
            let a = shadow_alpha(dx_out, dy_out);
            if a != 0 {
                let inv = 255 - a;
                let o = px as usize * 4;
                *row.add(o)     = ((*row.add(o)     as u32 * inv) / 255) as u8;
                *row.add(o + 1) = ((*row.add(o + 1) as u32 * inv) / 255) as u8;
                *row.add(o + 2) = ((*row.add(o + 2) as u32 * inv) / 255) as u8;
                // o+3 is the ignored X byte of the RGBX back-buffer — leave it.
            }
            px += 1;
        }
        py += 1;
    }
}
