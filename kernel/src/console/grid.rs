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
    cells:    Vec<Cell>,       // len == cols*rows, row-major
    cur_col:  u16,
    cur_row:  u16,
    fg:       Rgb,
    bg:       Rgb,
    attr:     CellAttr,
    dirty:    Vec<(u16, u16)>, // len == rows
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

    /// Resetta tutte le righe a pulite. Chiamato dal render dopo il blit.
    pub fn clear_dirty(&mut self) {
        for d in self.dirty.iter_mut() { *d = CLEAN; }
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
        if self.cur_row + 1 >= self.rows {
            self.scroll_up();
        } else {
            self.cur_row += 1;
        }
    }

    pub fn scroll_up(&mut self) {
        let cols = self.cols as usize;
        let rows = self.rows as usize;
        // Sposta le righe 1..rows in 0..rows-1.
        self.cells.copy_within(cols..rows * cols, 0);
        // Svuota l'ultima riga con i colori correnti.
        let blank = Cell::blank(self.fg, self.bg);
        let last = (rows - 1) * cols;
        for c in self.cells[last..].iter_mut() { *c = blank; }
        self.cur_row = self.rows - 1;
        // Tutto lo schermo è cambiato.
        for d in self.dirty.iter_mut() { *d = (0, self.cols - 1); }
    }

    pub fn clear(&mut self) {
        let blank = Cell::blank(self.fg, self.bg);
        for c in self.cells.iter_mut() { *c = blank; }
        self.cur_col = 0;
        self.cur_row = 0;
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
