//! Noto Sans Mono Size 24 lookup wrapper. Falls back to '?' on missing glyphs.

use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight, RasterizedChar};

pub const FONT_HEIGHT: RasterHeight = RasterHeight::Size24;
pub const FONT_WEIGHT: FontWeight   = FontWeight::Regular;

/// Glyph cell width in pixels. In noto-sans-mono-bitmap 0.3 this is
/// constant for a given (weight, height); read it once per call.
pub const fn glyph_width() -> usize {
    get_raster_width(FONT_WEIGHT, FONT_HEIGHT)
}

pub const fn glyph_height() -> usize {
    FONT_HEIGHT.val()
}

const FALLBACK: char = '?';

/// Picks the weight (Bold if `bold`). Falls back to '?'
/// in the same weight, then '?' Regular.
pub fn raster_for_weight(ch: char, bold: bool) -> RasterizedChar {
    let w = if bold { FontWeight::Bold } else { FontWeight::Regular };
    get_raster(ch, w, FONT_HEIGHT)
        .or_else(|| get_raster(FALLBACK, w, FONT_HEIGHT))
        .or_else(|| get_raster(FALLBACK, FontWeight::Regular, FONT_HEIGHT))
        .expect("noto fallback '?' missing")
}
