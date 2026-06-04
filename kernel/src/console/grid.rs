//! Griglia di celle del terminale. Conosce char/colori/attributi, cursore e
//! quali celle sono "dirty" (da ridisegnare). Non conosce i pixel: il
//! rendering vive in `render`. Single-thread (boot/console sotto Mutex).

use alloc::vec;
use alloc::vec::Vec;
use crate::console::ansi::{Cell, CellAttr, Rgb};

/// (min_col, max_col) inclusivi delle colonne dirty di una riga.
/// `None` codificato come (u16::MAX, 0): min>max ⇒ pulita.
const CLEAN: (u16, u16) = (u16::MAX, 0);

pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    cells:      Vec<Cell>,       // len == cols*rows, row-major
    cur_col:    u16,
    cur_row:    u16,
    fg:         Rgb,
    bg:         Rgb,
    attr:       CellAttr,
    dirty:      Vec<(u16, u16)>, // len == rows
    scroll_top: u16,
    scroll_bot: u16,
}

impl Grid {
    pub fn new(cols: u16, rows: u16, fg: Rgb, bg: Rgb) -> Self {
        let blank = Cell::blank(fg, bg);
        Grid {
            cols, rows,
            cells: vec![blank; (cols as usize) * (rows as usize)],
            cur_col: 0, cur_row: 0,
            fg, bg, attr: CellAttr::empty(),
            dirty: vec![CLEAN; rows as usize],
            scroll_top: 0,
            scroll_bot: rows - 1,
        }
    }

    #[inline]
    fn idx(&self, col: u16, row: u16) -> usize {
        (row as usize) * (self.cols as usize) + (col as usize)
    }

    pub fn cell(&self, col: u16, row: u16) -> Cell {
        self.cells[self.idx(col, row)]
    }

    pub fn cursor(&self) -> (u16, u16) { (self.cur_col, self.cur_row) }

    /// Span dirty della riga, o None se pulita.
    pub fn dirty_span(&self, row: u16) -> Option<(u16, u16)> {
        let (lo, hi) = self.dirty[row as usize];
        if lo > hi { None } else { Some((lo, hi)) }
    }

    pub fn set_fg(&mut self, fg: Rgb) { self.fg = fg; }
    pub fn set_bg(&mut self, bg: Rgb) { self.bg = bg; }
    pub fn set_attr(&mut self, attr: CellAttr) { self.attr = attr; }
    pub fn current_colors(&self) -> (Rgb, Rgb) { (self.fg, self.bg) }
    pub fn current_attr(&self) -> CellAttr { self.attr }

    fn mark(&mut self, col: u16, row: u16) {
        let (lo, hi) = self.dirty[row as usize];
        self.dirty[row as usize] = (lo.min(col), hi.max(col));
    }

    /// Forza dirty la singola cella (col, row). Clampa silenziosamente i
    /// valori fuori range — usato dal ghost-fix per l'ultima posizione del
    /// cursore, che potrebbe eccedere il grid dopo un alt-screen swap.
    pub fn mark_cell(&mut self, col: u16, row: u16) {
        if row < self.rows && col < self.cols { self.mark(col, row); }
    }

    /// Resetta tutte le righe a pulite. Chiamato dal render dopo il blit.
    pub fn clear_dirty(&mut self) {
        for d in self.dirty.iter_mut() { *d = CLEAN; }
    }

    /// Marca tutte le righe come dirty (full span). Usato al cambio di buffer.
    pub fn mark_all_dirty(&mut self) {
        for d in self.dirty.iter_mut() { *d = (0, self.cols - 1); }
    }

    /// Scrive il carattere visibile alla posizione cursore (colori/attr correnti)
    /// e avanza; wrap a fine riga.
    pub fn put(&mut self, ch: char) {
        if self.cur_col >= self.cols { self.newline(); }
        let (col, row) = (self.cur_col, self.cur_row);
        let i = self.idx(col, row);
        self.cells[i] = Cell { ch, fg: self.fg, bg: self.bg, attr: self.attr };
        self.mark(col, row);
        self.cur_col += 1;
    }

    pub fn cr(&mut self) { self.cur_col = 0; }

    pub fn bs(&mut self) {
        if self.cur_col > 0 { self.cur_col -= 1; }
    }

    pub fn tab(&mut self) {
        self.cur_col = (self.cur_col + 8) & !7;
        if self.cur_col >= self.cols { self.newline(); }
    }

    pub fn newline(&mut self) {
        self.cur_col = 0;
        if self.cur_row == self.scroll_bot {
            self.scroll_up();
        } else if self.cur_row + 1 < self.rows {
            self.cur_row += 1;
        }
    }

    pub fn scroll_up(&mut self) {
        let cols = self.cols as usize;
        let top = self.scroll_top as usize;
        let bot = self.scroll_bot as usize;
        let src = (top + 1) * cols;
        let end = (bot + 1) * cols;
        let dst = top * cols;
        self.cells.copy_within(src..end, dst);
        let blank = Cell::blank(self.fg, self.bg);
        let last = bot * cols;
        for c in self.cells[last..last + cols].iter_mut() { *c = blank; }
        self.cur_row = self.scroll_bot;
        for r in top..=bot { self.dirty[r] = (0, self.cols - 1); }
    }

    pub fn set_scroll_region(&mut self, top: u16, bot: u16) {
        if top < bot && bot < self.rows {
            self.scroll_top = top;
            self.scroll_bot = bot;
        } else {
            self.scroll_top = 0;
            self.scroll_bot = self.rows - 1;
        }
        self.cur_col = 0;
        self.cur_row = self.scroll_top;
    }

    pub fn clear(&mut self) {
        let blank = Cell::blank(self.fg, self.bg);
        for c in self.cells.iter_mut() { *c = blank; }
        self.cur_col = 0;
        self.cur_row = 0;
        self.scroll_top = 0;
        self.scroll_bot = self.rows - 1;
        for d in self.dirty.iter_mut() { *d = (0, self.cols - 1); }
    }

    pub fn move_up(&mut self, n: u16)    { self.cur_row = self.cur_row.saturating_sub(n); }
    pub fn move_down(&mut self, n: u16)  { self.cur_row = (self.cur_row + n).min(self.rows - 1); }
    pub fn move_left(&mut self, n: u16)  { self.cur_col = self.cur_col.saturating_sub(n); }
    pub fn move_right(&mut self, n: u16) { self.cur_col = (self.cur_col + n).min(self.cols - 1); }

    pub fn goto(&mut self, col: u16, row: u16) {
        self.cur_col = col.min(self.cols - 1);
        self.cur_row = row.min(self.rows - 1);
    }

    /// Cancella dalla colonna del cursore a fine riga (riempi di blank).
    pub fn erase_to_eol(&mut self) {
        let (col, row) = (self.cur_col, self.cur_row);
        let blank = Cell::blank(self.fg, self.bg);
        for c in col..self.cols {
            let i = self.idx(c, row);
            self.cells[i] = blank;
        }
        let hi = self.cols - 1;
        let (lo, _) = self.dirty[row as usize];
        self.dirty[row as usize] = (lo.min(col), hi);
    }
}
