//! Cache di maschere di copertura alpha, indicizzate per (char, bold).
//! Una maschera è un buffer flat `w*h` di intensità 0..255 (row-major),
//! ricavato una volta da `font::raster_for` e poi riusato. Comporre/colorare
//! avviene altrove (render), così il truecolor non moltiplica le entry.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use crate::console::font::{glyph_height, glyph_width, raster_for};

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

    /// Ritorna la maschera per `ch`. `bold` è accettato ora ma nel Plan 1 usa
    /// sempre il peso Regular (il peso Bold arriva nel Plan 2). Il flag entra
    /// comunque nella chiave per non invalidare la cache più avanti.
    pub fn mask(&mut self, ch: char, bold: bool) -> &GlyphMask {
        self.map.entry((ch, bold)).or_insert_with(|| rasterize(ch))
    }
}

fn rasterize(ch: char) -> GlyphMask {
    let w = glyph_width();
    let h = glyph_height();
    let mut alpha = vec![0u8; w * h];
    let r = raster_for(ch);
    for (ry, line) in r.raster().iter().enumerate() {
        if ry >= h { break; }
        for (rx, &intensity) in line.iter().enumerate() {
            if rx >= w { break; }
            alpha[ry * w + rx] = intensity;
        }
    }
    GlyphMask { w, h, alpha }
}
