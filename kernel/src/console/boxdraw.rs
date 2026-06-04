//! Glifi box-drawing procedurali (U+2500–257F). Noto Size24 non include il
//! blocco Box Drawing, quindi i bordi ratatui cadrebbero su '?'. Renderizziamo
//! i caratteri usati dai bordi ratatui (light/heavy/double: linee, angoli, tee,
//! croce) in una maschera alpha. Gli angoli arrotondati (╭╮╰╯) sono resi come
//! angoli light netti (arco vero = follow-up minore).

use alloc::vec;
use crate::console::font::{glyph_height, glyph_width};
use crate::console::glyphcache::GlyphMask;

// Peso per braccio: 0 nessuno, 1 light, 2 heavy, 3 double.
struct Arms { up: u8, down: u8, left: u8, right: u8 }

fn arms(ch: char) -> Option<Arms> {
    let a = |up, down, left, right| Some(Arms { up, down, left, right });
    match ch {
        '\u{2500}' => a(0,0,1,1), '\u{2501}' => a(0,0,2,2),
        '\u{2502}' => a(1,1,0,0), '\u{2503}' => a(2,2,0,0),
        '\u{250C}' => a(0,1,0,1), '\u{250F}' => a(0,2,0,2),
        '\u{2510}' => a(0,1,1,0), '\u{2513}' => a(0,2,2,0),
        '\u{2514}' => a(1,0,0,1), '\u{2517}' => a(2,0,0,2),
        '\u{2518}' => a(1,0,1,0), '\u{251B}' => a(2,0,2,0),
        '\u{251C}' => a(1,1,0,1), '\u{2523}' => a(2,2,0,2),
        '\u{2524}' => a(1,1,1,0), '\u{252B}' => a(2,2,2,0),
        '\u{252C}' => a(0,1,1,1), '\u{2533}' => a(0,2,2,2),
        '\u{2534}' => a(1,0,1,1), '\u{253B}' => a(2,0,2,2),
        '\u{253C}' => a(1,1,1,1), '\u{254B}' => a(2,2,2,2),
        '\u{2550}' => a(0,0,3,3), '\u{2551}' => a(3,3,0,0),
        '\u{2554}' => a(0,3,0,3), '\u{2557}' => a(0,3,3,0),
        '\u{255A}' => a(3,0,0,3), '\u{255D}' => a(3,0,3,0),
        '\u{2560}' => a(3,3,0,3), '\u{2563}' => a(3,3,3,0),
        '\u{2566}' => a(0,3,3,3), '\u{2569}' => a(3,0,3,3),
        '\u{256C}' => a(3,3,3,3),
        '\u{256D}' => a(0,1,0,1), '\u{256E}' => a(0,1,1,0),
        '\u{256F}' => a(1,0,1,0), '\u{2570}' => a(1,0,0,1),
        _ => None,
    }
}

#[inline]
fn put(a: &mut [u8], w: usize, h: usize, x: usize, y: usize) {
    if x < w && y < h { a[y * w + x] = 255; }
}

fn harm(a: &mut [u8], w: usize, h: usize, cy: usize, x0: usize, x1: usize, weight: u8) {
    let rows: &[i32] = match weight { 1 => &[0], 2 => &[-1,0,1], 3 => &[-2,2], _ => &[] };
    for &dy in rows {
        let y = cy as i32 + dy;
        if y < 0 { continue; }
        for x in x0..x1 { put(a, w, h, x, y as usize); }
    }
}

fn varm(a: &mut [u8], w: usize, h: usize, cx: usize, y0: usize, y1: usize, weight: u8) {
    let cols: &[i32] = match weight { 1 => &[0], 2 => &[-1,0,1], 3 => &[-2,2], _ => &[] };
    for &dx in cols {
        let x = cx as i32 + dx;
        if x < 0 { continue; }
        for y in y0..y1 { put(a, w, h, x as usize, y); }
    }
}

/// Maschera procedurale per un box-char, o None se non gestito.
pub fn mask(ch: char) -> Option<GlyphMask> {
    let d = arms(ch)?;
    let w = glyph_width();
    let h = glyph_height();
    let mut alpha = vec![0u8; w * h];
    let cx = w / 2;
    let cy = h / 2;
    harm(&mut alpha, w, h, cy, 0, cx + 1, d.left);
    harm(&mut alpha, w, h, cy, cx, w, d.right);
    varm(&mut alpha, w, h, cx, 0, cy + 1, d.up);
    varm(&mut alpha, w, h, cx, cy, h, d.down);
    Some(GlyphMask { w, h, alpha })
}
