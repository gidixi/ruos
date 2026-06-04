//! In-kernel self-test della console engine. Stampa un marker su seriale,
//! asserito da `make run-console-test`. Ogni task aggiunge asserzioni qui.

use crate::kprintln;

/// Esegue tutte le asserzioni. Stampa `CONSOLE_TEST: OK` se tutte passano,
/// altrimenti `CONSOLE_TEST: FAIL:<id>` al primo fallimento e ritorna.
pub fn run() {
    if let Err(id) = run_inner() {
        kprintln!("CONSOLE_TEST: FAIL:{}", id);
        return;
    }
    kprintln!("CONSOLE_TEST: OK");
}

fn run_inner() -> Result<(), u32> {
    // T1: harness vivo.
    check(1, 1 + 1 == 2)?;

    // T2: Cell default = spazio, attr vuoto, colori passati.
    {
        use crate::console::ansi::{Cell, CellAttr, WHITE, BLACK};
        let c = Cell::blank(WHITE, BLACK);
        check(2, c.ch == ' ' && c.fg == WHITE && c.bg == BLACK && c.attr.is_empty())?;
        let mut a = CellAttr::empty();
        a.insert(CellAttr::BOLD | CellAttr::REVERSE);
        check(3, a.contains(CellAttr::BOLD) && a.contains(CellAttr::REVERSE) && !a.contains(CellAttr::DIM))?;
    }

    // T4: la maschera di 'X' ha dimensioni di cella e qualche pixel acceso.
    {
        use crate::console::glyphcache::GlyphCache;
        use crate::console::font::{glyph_width, glyph_height};
        let mut gc = GlyphCache::new();
        let m = gc.mask('X', false);
        check(4, m.w == glyph_width() && m.h == glyph_height())?;
        check(5, m.alpha.iter().any(|&a| a > 0))?;
        // Lo spazio è tutto trasparente.
        let s = gc.mask(' ', false);
        check(6, s.alpha.iter().all(|&a| a == 0))?;
    }

    // T7: put avanza il cursore, scrive la cella, marca la riga dirty.
    {
        use crate::console::grid::Grid;
        use crate::console::ansi::{WHITE, BLACK};
        let mut g = Grid::new(10, 4, WHITE, BLACK);
        g.put('H'); g.put('i');
        check(7, g.cell(0, 0).ch == 'H' && g.cell(1, 0).ch == 'i')?;
        check(8, g.cursor() == (2, 0))?;
        let d = g.dirty_span(0);
        check(9, d == Some((0, 1)))?;
        g.cr(); check(10, g.cursor() == (0, 0))?;
        g.newline(); check(11, g.cursor() == (0, 1))?;
        g.bs(); check(12, g.cursor().0 == 0)?;
    }

    // T13: scroll fa salire le righe, l'ultima resta vuota, tutto dirty.
    {
        use crate::console::grid::Grid;
        use crate::console::ansi::{WHITE, BLACK};
        let mut g = Grid::new(4, 2, WHITE, BLACK);
        g.put('A'); g.newline(); // riga 0 = 'A', cursore a riga 1
        g.put('B'); g.newline(); // riga 1 = 'B', newline su ultima → scroll
        check(13, g.cell(0, 0).ch == 'B')?;
        check(14, g.cell(0, 1).ch == ' ')?;
        check(15, g.dirty_span(0).is_some() && g.dirty_span(1).is_some())?;
    }
    // T16: clear svuota tutto e azzera il cursore, marca dirty.
    {
        use crate::console::grid::Grid;
        use crate::console::ansi::{WHITE, BLACK};
        let mut g = Grid::new(4, 2, WHITE, BLACK);
        g.put('Z');
        g.clear();
        check(16, g.cell(0, 0).ch == ' ' && g.cursor() == (0, 0))?;
        check(17, g.dirty_span(0).is_some())?;
    }

    // T18: put_px nel back-buffer + read-back combaciano (BGR, 32bpp).
    {
        use crate::console::surface::Surface;
        use crate::console::fb::{FbInfo, PixelLayout};
        use crate::console::ansi::Rgb;
        let info = FbInfo { addr: core::ptr::null_mut(), width: 4, height: 2,
                            pitch: 16, bpp: 32, pixel: PixelLayout::Bgr };
        let mut s = Surface::new(info);
        let red = Rgb { r: 0xFF, g: 0x00, b: 0x00 };
        s.put_px(1, 1, red);
        check(18, s.read_px(1, 1) == red)?;
        check(19, s.read_px(0, 0) == Rgb { r: 0, g: 0, b: 0 })?;
    }

    // T20: render compone 'X' nel back-buffer; un pixel acceso della maschera
    // diventa fg, una cella vuota resta bg.
    {
        use crate::console::grid::Grid;
        use crate::console::surface::Surface;
        use crate::console::glyphcache::GlyphCache;
        use crate::console::render;
        use crate::console::fb::{FbInfo, PixelLayout};
        use crate::console::font::{glyph_width, glyph_height};
        use crate::console::ansi::Rgb;
        let fg = Rgb { r: 0xEE, g: 0xEE, b: 0xEE };
        let bg = Rgb { r: 0, g: 0, b: 0 };
        let gw = glyph_width() as u32; let gh = glyph_height() as u32;
        let info = FbInfo { addr: core::ptr::null_mut(), width: gw * 2, height: gh,
                            pitch: (gw * 2 * 4), bpp: 32, pixel: PixelLayout::Bgr };
        let mut g = Grid::new(2, 1, fg, bg);
        let mut s = Surface::new(info);
        let mut gc = GlyphCache::new();
        g.put('X');
        render::flush(&mut g, &mut gc, &mut s);
        let m = gc.mask('X', false);
        let mut found = false;
        for y in 0..gh { for x in 0..gw {
            if m.alpha[(y as usize)*(gw as usize)+(x as usize)] == 255 {
                check(20, s.read_px(x, y) == fg)?; found = true; break;
            }
        } if found { break; } }
        check(21, found)?;
        check(22, g.dirty_span(0).is_none())?;
    }

    // T23: FramebufferConsole su FbInfo finto (addr null) — write_str aggiorna
    // la griglia e pubblica il cursore.
    #[cfg(feature = "boot-checks")]
    {
        use crate::console::fb::{FramebufferConsole, FbInfo, PixelLayout};
        use crate::console::ansi::{WHITE, BLACK};
        use crate::console::font::{glyph_width, glyph_height};
        let gw = glyph_width() as u32; let gh = glyph_height() as u32;
        let info = FbInfo { addr: core::ptr::null_mut(), width: gw*10, height: gh*3,
                            pitch: gw*10*4, bpp: 32, pixel: PixelLayout::Bgr };
        let mut con = FramebufferConsole::new(info, WHITE, BLACK);
        con.write_str("ok");
        check(23, con.cursor_for_test() == (2, 0))?;
    }

    // T24: full-redraw 80x25 sotto soglia. Misura puramente RAM (addr null),
    // quindi misura il costo compose; il costo blit MMIO si valuta a schermo.
    #[cfg(feature = "boot-checks")]
    {
        use crate::console::fb::{FramebufferConsole, FbInfo, PixelLayout};
        use crate::console::ansi::{WHITE, BLACK};
        use crate::console::font::{glyph_width, glyph_height};
        let gw = glyph_width() as u32; let gh = glyph_height() as u32;
        let info = FbInfo { addr: core::ptr::null_mut(), width: gw*80, height: gh*25,
                            pitch: gw*80*4, bpp: 32, pixel: PixelLayout::Bgr };
        let mut con = FramebufferConsole::new(info, WHITE, BLACK);
        let t0 = crate::boot::clock::read_tsc();
        for _ in 0..25 { con.write_str("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJ\n"); }
        let dt = crate::boot::clock::read_tsc().wrapping_sub(t0);
        crate::kprintln!("CONSOLE_PERF: full_redraw_tsc={}", dt);
        // Threshold calibrated on QEMU (measured ~820M cycles, 11×24 font,
        // cold GlyphCache, 2000 cells). 2B gives headroom on slow virt hosts
        // while still catching gross regressions (e.g. O(n²) recompose).
        check(24, dt < 2_000_000_000)?;
    }

    // T25-28: SGR truecolor + attributi + reset.
    {
        use crate::console::ansi::{apply_sgr, CellAttr, Rgb, WHITE, BLACK};
        let (fg, _b, _a) = apply_sgr([38u16,2,10,20,30].into_iter(), WHITE, BLACK, CellAttr::empty());
        check(25, fg == Rgb { r:10, g:20, b:30 })?;
        let (_f, bg, _a) = apply_sgr([48u16,2,7,8,9].into_iter(), WHITE, BLACK, CellAttr::empty());
        check(26, bg == Rgb { r:7, g:8, b:9 })?;
        let (_f,_b, a) = apply_sgr([1u16,4,7].into_iter(), WHITE, BLACK, CellAttr::empty());
        check(27, a.contains(CellAttr::BOLD) && a.contains(CellAttr::UNDERLINE) && a.contains(CellAttr::REVERSE))?;
        let (f2, b2, a2) = apply_sgr([0u16].into_iter(), Rgb{r:1,g:2,b:3}, Rgb{r:4,g:5,b:6}, CellAttr::BOLD);
        check(28, f2 == WHITE && b2 == BLACK && a2.is_empty())?;
    }

    // T29-30: reverse scambia fg/bg; dim scurisce fg.
    {
        use crate::console::grid::Grid; use crate::console::render;
        use crate::console::surface::Surface; use crate::console::glyphcache::GlyphCache;
        use crate::console::fb::{FbInfo, PixelLayout};
        use crate::console::ansi::{WHITE, BLACK, CellAttr};
        use crate::console::font::{glyph_width, glyph_height};
        let gw = glyph_width() as u32; let gh = glyph_height() as u32;
        let info = FbInfo { addr: core::ptr::null_mut(), width: gw, height: gh, pitch: gw*4, bpp: 32, pixel: PixelLayout::Bgr };
        let mut g = Grid::new(1, 1, WHITE, BLACK); let mut s = Surface::new(info); let mut gc = GlyphCache::new();
        g.set_attr(CellAttr::REVERSE); g.put('X');
        render::flush(&mut g, &mut gc, &mut s);
        let m = gc.mask('X', false);
        let mut hit = false;
        for y in 0..gh { for x in 0..gw {
            if m.alpha[(y as usize)*(gw as usize)+(x as usize)] == 255 { check(29, s.read_px(x,y) == BLACK)?; hit = true; break; }
        } if hit { break; } }
        let mut g2 = Grid::new(1,1,WHITE,BLACK); let mut s2 = Surface::new(info); let mut gc2 = GlyphCache::new();
        g2.set_attr(CellAttr::DIM); g2.put('X');
        render::flush(&mut g2, &mut gc2, &mut s2);
        let m2 = gc2.mask('X', false);
        for y in 0..gh { for x in 0..gw {
            if m2.alpha[(y as usize)*(gw as usize)+(x as usize)] == 255 {
                let px = s2.read_px(x,y);
                check(30, px != WHITE && px.r > 0)?; break;
            }
        } }
    }

    Ok(())
}

#[inline]
fn check(id: u32, cond: bool) -> Result<(), u32> {
    if cond { Ok(()) } else { Err(id) }
}
