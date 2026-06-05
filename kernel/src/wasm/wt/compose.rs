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
//! NOTE (SP4 / interface contract decision 6): a `WinDesc` describes ONE
//! DECORATED footprint produced by `Compositor::compose_window` (title bar +
//! [X] + surface), NOT a raw window surface. The band kernel paints a rect of
//! RGBA pixels at a screen (x,y); it does not care whether the source is a raw
//! surface or a decorated footprint, so this code is identical either way —
//! only the WinDesc CONSTRUCTION (in `wm.rs`) feeds it the decorated footprint
//! so the decorations survive parallel compositing.

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
