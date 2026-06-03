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

    Ok(())
}

#[inline]
fn check(id: u32, cond: bool) -> Result<(), u32> {
    if cond { Ok(()) } else { Err(id) }
}
