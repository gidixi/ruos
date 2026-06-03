//! Ponte griglia→pixel. Compone le celle dirty nel back-buffer della Surface
//! (maschera alpha × fg over bg) e blitta gli span dirty su MMIO.

use alloc::vec::Vec;
use crate::console::ansi::{Cell, Rgb};
use crate::console::font::{glyph_height, glyph_width};
use crate::console::glyphcache::GlyphCache;
use crate::console::grid::Grid;
use crate::console::surface::Surface;

/// Blend per canale `fg*α + bg*(1-α)`, α = intensity/255 (come fb.rs::blend).
#[inline]
fn blend(fg: Rgb, bg: Rgb, intensity: u8) -> Rgb {
    let a = intensity as u32;
    let ia = 255 - a;
    let mix = |f: u8, b: u8| (((f as u32) * a + (b as u32) * ia) / 255) as u8;
    Rgb { r: mix(fg.r, bg.r), g: mix(fg.g, bg.g), b: mix(fg.b, bg.b) }
}

/// Compone tutte le celle dirty nel back-buffer e blitta. Azzera il dirty.
pub fn flush(grid: &mut Grid, cache: &mut GlyphCache, surf: &mut Surface) {
    let gw = glyph_width() as u32;
    let gh = glyph_height() as u32;
    for row in 0..grid.rows {
        let Some((lo, hi)) = grid.dirty_span(row) else { continue };
        for col in lo..=hi {
            compose_cell(grid.cell(col, row), col as u32, row as u32, gw, gh, cache, surf);
        }
        let x0 = lo as u32 * gw;
        let x1 = (hi as u32 + 1) * gw - 1;
        let y0 = row as u32 * gh;
        let y1 = y0 + gh;
        surf.blit_rect(x0, x1, y0, y1);
    }
    grid.clear_dirty();
}

fn compose_cell(cell: Cell, col: u32, row: u32, gw: u32, gh: u32,
                cache: &mut GlyphCache, surf: &mut Surface) {
    let bold = cell.attr.contains(crate::console::ansi::CellAttr::BOLD);
    // Copy the alpha mask into a local Vec so that the &GlyphMask borrow on
    // `cache` ends before we start calling surf.put_px (which takes &mut surf).
    // Without the copy the borrow checker would see overlapping lifetimes on
    // the two mutable borrows inside this function.
    let (mask_w, alpha): (usize, Vec<u8>) = {
        let mask = cache.mask(cell.ch, bold);
        (mask.w, mask.alpha.clone())
    };
    let mask_h = if mask_w > 0 { alpha.len() / mask_w } else { 0 };
    let ox = col * gw;
    let oy = row * gh;
    for ry in 0..gh {
        for rx in 0..gw {
            let a = if (rx < mask_w as u32) && ((ry as usize) < mask_h) {
                alpha[(ry as usize) * mask_w + (rx as usize)]
            } else {
                0
            };
            let color = if a == 0 { cell.bg } else { blend(cell.fg, cell.bg, a) };
            surf.put_px(ox + rx, oy + ry, color);
        }
    }
}
