//! Cache di maschere di copertura alpha, indicizzate per (char, bold).
//! Una maschera è un buffer flat `w*h` di intensità 0..255 (row-major),
//! ricavato una volta da `font::raster_for` e poi riusato. Comporre/colorare
//! avviene altrove (render), così il truecolor non moltiplica le entry.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use crate::console::font::{glyph_height, glyph_width};

pub struct GlyphMask {
    pub w: usize,
    pub h: usize,
    pub alpha: Vec<u8>, // len == w*h, row-major
}

pub struct GlyphCache {
    map: BTreeMap<(char, bool), GlyphMask>,
}

impl GlyphCache {
    pub fn new() -> Self {
        GlyphCache { map: BTreeMap::new() }
    }

    /// Ritorna la maschera per `ch`. Usa il peso Bold se `bold`, Regular altrimenti.
    pub fn mask(&mut self, ch: char, bold: bool) -> &GlyphMask {
        self.map.entry((ch, bold)).or_insert_with(|| rasterize(ch, bold))
    }

    /// Pre-rasterizza tutto l'ASCII stampabile (0x20..=0x7E) nel peso Regular,
    /// così il render path (incluso quello del panic handler) è alloc-free per
    /// il testo ASCII: nessun cache-miss → nessuna alloc. Chiamato a init quando
    /// l'heap è sano.
    pub fn prewarm_ascii(&mut self) {
        for c in 0x20u8..=0x7E {
            let _ = self.mask(c as char, false);
        }
    }
}

fn rasterize(ch: char, bold: bool) -> GlyphMask {
    let w = glyph_width();
    let h = glyph_height();
    let mut alpha = vec![0u8; w * h];
    let r = crate::console::font::raster_for_weight(ch, bold);
    for (ry, line) in r.raster().iter().enumerate() {
        if ry >= h { break; }
        for (rx, &intensity) in line.iter().enumerate() {
            if rx >= w { break; }
            alpha[ry * w + rx] = intensity;
        }
    }
    GlyphMask { w, h, alpha }
}
