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

    Ok(())
}

#[inline]
fn check(id: u32, cond: bool) -> Result<(), u32> {
    if cond { Ok(()) } else { Err(id) }
}
