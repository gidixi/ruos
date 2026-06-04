//! Ponte griglia→pixel. Compone le celle dirty nel back-buffer della Surface
//! (maschera alpha × fg over bg) e blitta gli span dirty su MMIO.

use crate::console::ansi::{Cell, CellAttr, Rgb};
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

fn dim(fg: Rgb, bg: Rgb) -> Rgb { blend(fg, bg, 160) } // ~63% fg toward bg

fn compose_cell(cell: Cell, col: u32, row: u32, gw: u32, gh: u32,
                cache: &mut GlyphCache, surf: &mut Surface) {
    let bold = cell.attr.contains(CellAttr::BOLD);
    let (mut fg, bg) = if cell.attr.contains(CellAttr::REVERSE) {
        (cell.bg, cell.fg)
    } else {
        (cell.fg, cell.bg)
    };
    if cell.attr.contains(CellAttr::DIM) { fg = dim(fg, bg); }
    let mask = cache.mask(cell.ch, bold);
    let ox = col * gw;
    let oy = row * gh;
    let w = mask.w as u32;
    for ry in 0..gh {
        for rx in 0..gw {
            let alpha = if (rx < w) && ((ry as usize) < mask.h) {
                mask.alpha[(ry as usize) * mask.w + (rx as usize)]
            } else { 0 };
            let color = if alpha == 0 { bg } else { blend(fg, bg, alpha) };
            surf.put_px(ox + rx, oy + ry, color);
        }
    }
}
