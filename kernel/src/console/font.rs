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

pub fn raster_for(ch: char) -> RasterizedChar {
    get_raster(ch, FONT_WEIGHT, FONT_HEIGHT)
        .unwrap_or_else(|| get_raster(FALLBACK, FONT_WEIGHT, FONT_HEIGHT)
            .expect("noto fallback '?' missing"))
}
