# Terminal Engine — Plan 1: Foundation (back-buffer + dirty blit)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Sostituire il rendering "scrittura diretta su MMIO per glifo" della console framebuffer con un'architettura back-buffer + griglia di celle + blit dirty, mantenendo identico il comportamento visibile, eliminando flicker e accelerando i redraw.

**Architecture:** Buffer ibrido C dallo spec. La `Grid` (celle char/fg/bg/attr) assorbe il parsing vte e marca le celle dirty. La `Surface` possiede un pixel back-buffer in RAM con lo stesso layout byte del framebuffer Limine; `render::flush` compone le celle dirty nel back-buffer (via maschere alpha da `GlyphCache`) e blitta solo gli span dirty su MMIO. `FramebufferConsole` orchestra: `write_str` → parse vte → aggiorna Grid → `flush`. Il blink cursore resta nell'IRQ timer via atomics (nessun lock in IRQ).

**Tech Stack:** Rust nightly `no_std` + `alloc`, target `x86_64-unknown-none`. Crate esistenti: `vte`, `noto-sans-mono-bitmap 0.3` (size_24), `bitflags 2`, `spin 0.9`. Build/test via WSL Ubuntu. Harness: self-test in-kernel + marker seriale asseriti da `make run-console-test`.

**Prerequisito:** branch `feature/terminal-engine` (spec già committato `b15a450`).

**Build/run (sempre via WSL):**
- Compila: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso'`
- Test console: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-console-test'`
- Boot test esistente (regressione): `... && make run-test`

**Convenzioni di test:** TDD via marker seriale. Ogni task aggiunge asserzioni a `console::engine_test::run()`, che stampa `CONSOLE_TEST: OK` se tutte passano o `CONSOLE_TEST: FAIL:<id>` al primo fallimento. `make run-console-test` builda l'ISO, boota QEMU headless, e asserisce `CONSOLE_TEST: OK`. Loop rosso→verde: aggiungi asserzione (o tipo nuovo) → il build fallisce o stampa FAIL → implementa → OK → commit.

**Nota CLAUDE.md:** per ogni task creare la entry `CHANGELOG/NN-26-06-03-<slug>.md`. Ultimi usati: 223 (spec), 224 (questo piano). Le entry dei task partono da **225**; gli NN scritti nei task sono indicativi (es. `224-...` → usa 225, e a salire) — **verifica sempre** con `ls CHANGELOG/` e usa il prossimo libero. Commit solo su `feature/terminal-engine`. Niente push salvo richiesta.

---

## File Structure

| File | Stato | Responsabilità |
|---|---|---|
| `kernel/src/console/ansi.rs` | modify | aggiunge `CellAttr` (bitflags) e `Cell` ai tipi colore esistenti |
| `kernel/src/console/glyphcache.rs` | create | cache maschere alpha `(char,bold) → GlyphMask` da `raster_for` |
| `kernel/src/console/grid.rs` | create | griglia celle + cursore + dirty tracking + scroll/clear; parsing-agnostica dei pixel |
| `kernel/src/console/surface.rs` | create | pixel back-buffer RAM (layout = FB) + blit span dirty su MMIO + misura WC |
| `kernel/src/console/render.rs` | create | `flush`: compone celle dirty nel back-buffer e blitta gli span |
| `kernel/src/console/fb.rs` | modify | `FramebufferConsole` ricostruito su Grid+Surface+GlyphCache+render; API pubblica invariata; blink IRQ invariato |
| `kernel/src/console/engine_test.rs` | create | self-test in-kernel, marker seriale |
| `kernel/src/console/mod.rs` | modify | dichiara i nuovi moduli; trait `Console` invariato |
| `kernel/src/boot/phases/devices.rs` | modify | invoca `engine_test::run()` dopo il self-test FB esistente |
| `Makefile` | modify | target `run-console-test` |
| `user-bin/console-test-init.sh` | create | INIT_SCRIPT del test (boot → marker → poweroff) |

Confini: `grid` non tocca pixel; `surface` non conosce celle/char; `render` è l'unico ponte. `glyphcache` dipende solo da `font`. Ognuno self-test-abile.

---

## Task 1: Test harness (marker seriale + make target)

**Files:**
- Create: `kernel/src/console/engine_test.rs`
- Modify: `kernel/src/console/mod.rs`
- Modify: `kernel/src/boot/phases/devices.rs:13`
- Create: `user-bin/console-test-init.sh`
- Modify: `Makefile`

- [ ] **Step 1: Crea il self-test con una asserzione banale**

`kernel/src/console/engine_test.rs`:
```rust
//! In-kernel self-test della console engine. Stampa un marker su seriale,
//! asserito da `make run-console-test`. Ogni task aggiunge asserzioni qui.

use crate::serial_println;

/// Esegue tutte le asserzioni. Stampa `CONSOLE_TEST: OK` se tutte passano,
/// altrimenti `CONSOLE_TEST: FAIL:<id>` al primo fallimento e ritorna.
pub fn run() {
    if let Err(id) = run_inner() {
        serial_println!("CONSOLE_TEST: FAIL:{}", id);
        return;
    }
    serial_println!("CONSOLE_TEST: OK");
}

fn run_inner() -> Result<(), u32> {
    // T1: harness vivo.
    check(1, 1 + 1 == 2)?;
    Ok(())
}

#[inline]
fn check(id: u32, cond: bool) -> Result<(), u32> {
    if cond { Ok(()) } else { Err(id) }
}
```

Nota: verifica il nome reale della macro di stampa seriale (cerca `serial_println` o `kprintln` nel crate; usa quella esistente). Se la macro è `crate::serial_println`, l'import sopra va bene; altrimenti adatta.

- [ ] **Step 2: Dichiara il modulo**

In `kernel/src/console/mod.rs`, dopo `pub mod fb_init;` aggiungi:
```rust
pub mod engine_test;
```

- [ ] **Step 3: Invoca al boot dopo il self-test FB**

In `kernel/src/boot/phases/devices.rs`, subito dopo la riga 13 (`let ok = crate::console::fb::self_test(&mut fb);` e relativo log), aggiungi:
```rust
        crate::console::engine_test::run();
```
Inseriscila dove `fb` è ancora disponibile / prima che venga `attach`-ato non importa: `engine_test::run()` costruisce le sue strutture, non usa `fb`. Mettila subito dopo il log del self-test FB.

- [ ] **Step 4: INIT_SCRIPT del test**

`user-bin/console-test-init.sh` (deve solo far bootare fino al marker e poi spegnere):
```sh
poweroff
```
(La `engine_test::run()` gira in fase devices, prima della shell; il marker è già emesso quando init parte. `poweroff` chiude QEMU.)

- [ ] **Step 5: Target Makefile**

Studia un target esistente analogo (es. `run-rtop-test` o `run-usb-key-test`) per copiare lo stile esatto (flag QEMU, redirezione log, grep marker, isa-debug-exit). Aggiungi in `Makefile`:
```make
# Console engine self-test: boots, engine_test::run() emette CONSOLE_TEST: OK
# su seriale in fase devices, poi poweroff.
run-console-test: limine $(USER_WASMS)
	@$(MAKE) iso INIT_SCRIPT=user-bin/console-test-init.sh > build/console-iso.log 2>&1 || { echo TEST_FAIL_ISO; tail -20 build/console-iso.log; exit 1; }
	@timeout 90 qemu-system-x86_64 $(QEMU_TEST_FLAGS) -serial stdio -display none 2>&1 | tee build/console-test.log | grep -q 'CONSOLE_TEST: OK' && echo CONSOLE_TEST_PASS || { echo CONSOLE_TEST_FAIL; tail -40 build/console-test.log; exit 1; }
```
Adatta `$(QEMU_TEST_FLAGS)`/nome variabili a quelle realmente definite nel Makefile (cerca come `run-rtop-test` invoca QEMU; riusa le stesse variabili). Il punto fermo: build ISO con quell'INIT_SCRIPT, boot headless, `grep -q 'CONSOLE_TEST: OK'`.

- [ ] **Step 6: Esegui — deve passare (harness vivo)**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-console-test'`
Expected: stampa `CONSOLE_TEST_PASS`. (Conferma che harness, boot hook, marker e grep funzionano prima di costruire feature.)

- [ ] **Step 7: Commit**

```bash
git add kernel/src/console/engine_test.rs kernel/src/console/mod.rs kernel/src/boot/phases/devices.rs user-bin/console-test-init.sh Makefile CHANGELOG/224-26-06-03-console-test-harness.md
git commit -m "test(console): engine self-test harness + run-console-test target"
```
(Crea prima la entry CHANGELOG 224.)

---

## Task 2: Tipi `Cell` e `CellAttr`

**Files:**
- Modify: `kernel/src/console/ansi.rs` (append in fondo)
- Modify: `kernel/src/console/engine_test.rs`

- [ ] **Step 1: Asserzione (rosso)**

In `engine_test.rs`, dentro `run_inner()` prima di `Ok(())`:
```rust
    // T2: Cell default = spazio, attr vuoto, colori passati.
    {
        use crate::console::ansi::{Cell, CellAttr, WHITE, BLACK};
        let c = Cell::blank(WHITE, BLACK);
        check(2, c.ch == ' ' && c.fg == WHITE && c.bg == BLACK && c.attr.is_empty())?;
        let mut a = CellAttr::empty();
        a.insert(CellAttr::BOLD | CellAttr::REVERSE);
        check(3, a.contains(CellAttr::BOLD) && a.contains(CellAttr::REVERSE) && !a.contains(CellAttr::DIM))?;
    }
```

- [ ] **Step 2: Compila — deve fallire**

Run: `... && make iso`
Expected: errore di compilazione `cannot find ... Cell` / `CellAttr` in `ansi`.

- [ ] **Step 3: Implementa i tipi**

In fondo a `kernel/src/console/ansi.rs`:
```rust
use bitflags::bitflags;

bitflags! {
    /// Attributi testo per cella. Bold/dim/underline/reverse sono definiti ora
    /// per stabilizzare il tipo; il rendering degli attributi arriva nel Plan 2.
    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    pub struct CellAttr: u8 {
        const BOLD      = 0b0001;
        const DIM       = 0b0010;
        const UNDERLINE = 0b0100;
        const REVERSE   = 0b1000;
    }
}

/// Una cella della griglia terminale.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Cell {
    pub ch:   char,
    pub fg:   Rgb,
    pub bg:   Rgb,
    pub attr: CellAttr,
}

impl Cell {
    /// Cella vuota (spazio) con i colori dati e nessun attributo.
    pub fn blank(fg: Rgb, bg: Rgb) -> Self {
        Cell { ch: ' ', fg, bg, attr: CellAttr::empty() }
    }
}
```
Nota: `Rgb` deriva già `PartialEq, Eq` (verifica in cima a `ansi.rs`; lo fa). `bitflags 2` richiede il `#[derive(...)]` esplicito dentro la macro come sopra.

- [ ] **Step 4: Esegui — verde**

Run: `... && make run-console-test`
Expected: `CONSOLE_TEST_PASS`.

- [ ] **Step 5: Commit**

```bash
git add kernel/src/console/ansi.rs kernel/src/console/engine_test.rs CHANGELOG/225-26-06-03-cell-types.md
git commit -m "feat(console): Cell + CellAttr types"
```

---

## Task 3: `GlyphCache` (maschere alpha)

**Files:**
- Create: `kernel/src/console/glyphcache.rs`
- Modify: `kernel/src/console/mod.rs`
- Modify: `kernel/src/console/engine_test.rs`

- [ ] **Step 1: Asserzione (rosso)**

In `engine_test.rs` dentro `run_inner()`:
```rust
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
```

- [ ] **Step 2: Compila — fallisce**

Run: `... && make iso`
Expected: `cannot find module glyphcache` / `GlyphCache`.

- [ ] **Step 3: Implementa**

`kernel/src/console/glyphcache.rs`:
```rust
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
    // raster() = righe di intensità; per Noto mono la dimensione combacia con
    // la cella, ma clamp per sicurezza.
    for (ry, line) in r.raster().iter().enumerate() {
        if ry >= h { break; }
        for (rx, &intensity) in line.iter().enumerate() {
            if rx >= w { break; }
            alpha[ry * w + rx] = intensity;
        }
    }
    GlyphMask { w, h, alpha }
}
```

- [ ] **Step 4: Dichiara il modulo**

In `kernel/src/console/mod.rs`, accanto agli altri `pub mod`:
```rust
pub mod glyphcache;
```

- [ ] **Step 5: Esegui — verde**

Run: `... && make run-console-test`
Expected: `CONSOLE_TEST_PASS`.

- [ ] **Step 6: Commit**

```bash
git add kernel/src/console/glyphcache.rs kernel/src/console/mod.rs kernel/src/console/engine_test.rs CHANGELOG/226-26-06-03-glyph-cache.md
git commit -m "feat(console): alpha-mask glyph cache"
```

---

## Task 4: `Grid` core (celle + cursore + dirty + put/cr/bs/tab)

**Files:**
- Create: `kernel/src/console/grid.rs`
- Modify: `kernel/src/console/mod.rs`
- Modify: `kernel/src/console/engine_test.rs`

- [ ] **Step 1: Asserzione (rosso)**

In `engine_test.rs`:
```rust
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
        // CR riporta colonna a 0, '\n' (newline) scende riga.
        g.cr(); check(10, g.cursor() == (0, 0))?;
        g.newline(); check(11, g.cursor() == (0, 1))?;
        // backspace su col 0 di riga 1 non va sotto 0.
        g.bs(); check(12, g.cursor().0 == 0)?;
    }
```

- [ ] **Step 2: Compila — fallisce**

Run: `... && make iso`
Expected: `cannot find module grid`.

- [ ] **Step 3: Implementa**

`kernel/src/console/grid.rs`:
```rust
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

    // scroll_up + clear sono nel Task 5.
}
```

- [ ] **Step 4: Dichiara il modulo**

In `kernel/src/console/mod.rs`:
```rust
pub mod grid;
```

- [ ] **Step 5: Stub temporaneo per `scroll_up`**

Per compilare il Task 4 prima del Task 5, aggiungi uno stub provvisorio dentro `impl Grid` (verrà sostituito nel Task 5):
```rust
    pub fn scroll_up(&mut self) {
        // Task 5: implementazione reale. Stub: tieni il cursore sull'ultima riga.
        self.cur_row = self.rows - 1;
    }
```

- [ ] **Step 6: Esegui — verde**

Run: `... && make run-console-test`
Expected: `CONSOLE_TEST_PASS`.

- [ ] **Step 7: Commit**

```bash
git add kernel/src/console/grid.rs kernel/src/console/mod.rs kernel/src/console/engine_test.rs CHANGELOG/227-26-06-03-grid-core.md
git commit -m "feat(console): grid core — cells, cursor, dirty tracking"
```

---

## Task 5: `Grid` scroll + clear

**Files:**
- Modify: `kernel/src/console/grid.rs`
- Modify: `kernel/src/console/engine_test.rs`

- [ ] **Step 1: Asserzione (rosso)**

In `engine_test.rs`:
```rust
    // T13: scroll fa salire le righe, l'ultima resta vuota, tutto dirty.
    {
        use crate::console::grid::Grid;
        use crate::console::ansi::{WHITE, BLACK};
        let mut g = Grid::new(4, 2, WHITE, BLACK);
        g.put('A'); g.newline(); // riga 0 = 'A', cursore a riga 1
        g.put('B'); g.newline(); // riga 1 = 'B', newline su ultima → scroll
        // dopo scroll: riga 0 = 'B', riga 1 vuota
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
```

- [ ] **Step 2: Compila/esegui — fallisce**

Run: `... && make run-console-test`
Expected: con lo stub di scroll del Task 4, `CONSOLE_TEST: FAIL:13` (riga 0 non è 'B'). `clear` non esiste → in realtà errore di compilazione `no method clear`; risolvi entrambi nello Step 3.

- [ ] **Step 3: Implementa scroll_up + clear (sostituisci lo stub)**

In `grid.rs`, rimpiazza lo stub `scroll_up` e aggiungi `clear`:
```rust
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
```

- [ ] **Step 4: Esegui — verde**

Run: `... && make run-console-test`
Expected: `CONSOLE_TEST_PASS`.

- [ ] **Step 5: Commit**

```bash
git add kernel/src/console/grid.rs kernel/src/console/engine_test.rs CHANGELOG/228-26-06-03-grid-scroll-clear.md
git commit -m "feat(console): grid scroll_up + clear"
```

---

## Task 6: `Surface` (pixel back-buffer + blit + misura WC)

**Files:**
- Create: `kernel/src/console/surface.rs`
- Modify: `kernel/src/console/mod.rs`
- Modify: `kernel/src/console/engine_test.rs`

La `Surface` possiede un back-buffer RAM con lo **stesso layout byte** del framebuffer (pitch, bpp, pixel layout di `FbInfo`), così il blit è una copia di span contigui. Riusa la logica di packing pixel da `fb.rs::pixel_write`.

- [ ] **Step 1: Asserzione (rosso)**

In `engine_test.rs` — questo test richiede un `FbInfo`. Usane uno sintetico in RAM (non il FB reale) per testare la logica back-buffer in isolamento:
```rust
    // T18: put_px nel back-buffer + read-back combaciano (BGR, 32bpp).
    {
        use crate::console::surface::Surface;
        use crate::console::fb::{FbInfo, PixelLayout};
        use crate::console::ansi::Rgb;
        // FbInfo finto: addr null (non blittiamo su MMIO in questo test), 4x2, 32bpp.
        let info = FbInfo { addr: core::ptr::null_mut(), width: 4, height: 2,
                            pitch: 16, bpp: 32, pixel: PixelLayout::Bgr };
        let mut s = Surface::new(info);
        let red = Rgb { r: 0xFF, g: 0x00, b: 0x00 };
        s.put_px(1, 1, red);
        check(18, s.read_px(1, 1) == red)?;
        check(19, s.read_px(0, 0) == Rgb { r: 0, g: 0, b: 0 })?;
    }
```

- [ ] **Step 2: Compila — fallisce**

Run: `... && make iso`
Expected: `cannot find module surface`.

- [ ] **Step 3: Implementa**

`kernel/src/console/surface.rs`:
```rust
//! Pixel back-buffer in RAM con layout identico al framebuffer + blit degli
//! span dirty su MMIO. È l'unica unità che scrive sul framebuffer.

use core::ptr::write_volatile;
use alloc::vec;
use alloc::vec::Vec;
use crate::console::ansi::Rgb;
use crate::console::fb::{FbInfo, PixelLayout};

pub struct Surface {
    info: FbInfo,
    back: Vec<u8>, // len == pitch*height, layout = framebuffer
}

impl Surface {
    pub fn new(info: FbInfo) -> Self {
        let len = (info.pitch as usize) * (info.height as usize);
        Surface { info, back: vec![0u8; len] }
    }

    #[inline]
    fn bpp_bytes(&self) -> usize { (self.info.bpp as usize) / 8 }

    #[inline]
    fn offset(&self, x: u32, y: u32) -> usize {
        (y as usize) * (self.info.pitch as usize) + (x as usize) * self.bpp_bytes()
    }

    #[inline]
    fn pack(&self, c: Rgb) -> (u8, u8, u8) {
        match self.info.pixel {
            PixelLayout::Bgr => (c.b, c.g, c.r),
            PixelLayout::Rgb => (c.r, c.g, c.b),
        }
    }

    /// Scrive un pixel SOLO nel back-buffer.
    pub fn put_px(&mut self, x: u32, y: u32, c: Rgb) {
        if x >= self.info.width || y >= self.info.height { return; }
        let off = self.offset(x, y);
        let (b0, b1, b2) = self.pack(c);
        self.back[off] = b0;
        self.back[off + 1] = b1;
        self.back[off + 2] = b2;
        if self.bpp_bytes() == 4 { self.back[off + 3] = 0; }
    }

    /// Rilegge un pixel dal back-buffer (per i test / debug).
    pub fn read_px(&self, x: u32, y: u32) -> Rgb {
        let off = self.offset(x, y);
        let (b0, b1, b2) = (self.back[off], self.back[off + 1], self.back[off + 2]);
        match self.info.pixel {
            PixelLayout::Bgr => Rgb { r: b2, g: b1, b: b0 },
            PixelLayout::Rgb => Rgb { r: b0, g: b1, b: b2 },
        }
    }

    /// Blitta su MMIO le righe `y0..y1` per le colonne `x0..=x1` (uno span
    /// contiguo per riga). No-op se addr è null (test in RAM).
    pub fn blit_rect(&self, x0: u32, x1: u32, y0: u32, y1: u32) {
        if self.info.addr.is_null() { return; }
        let bpp = self.bpp_bytes();
        let pitch = self.info.pitch as usize;
        let xa = x0.min(self.info.width.saturating_sub(1)) as usize;
        let xb = (x1.min(self.info.width.saturating_sub(1)) as usize) + 1;
        let span = (xb - xa) * bpp;
        for y in y0..y1.min(self.info.height) {
            let off = (y as usize) * pitch + xa * bpp;
            // SAFETY: off..off+span dentro il mapping FB (clamp sopra). Copia
            // back-buffer→MMIO. write_volatile per byte garantisce gli store su MMIO.
            unsafe {
                let src = self.back.as_ptr().add(off);
                let dst = self.info.addr.add(off);
                let mut i = 0;
                while i < span {
                    write_volatile(dst.add(i), *src.add(i));
                    i += 1;
                }
            }
        }
    }

    pub fn info(&self) -> FbInfo { self.info }
}
```
Nota perf: il blit per-byte è semplice e corretto; se la misura TSC (Task 9) lo mostra lento, l'ottimizzazione è `copy_nonoverlapping` per span (un memcpy) — ma su MMIO `write_volatile` è più sicuro. Decidi in Task 9 sulla base del numero.

- [ ] **Step 4: Dichiara il modulo**

In `kernel/src/console/mod.rs`:
```rust
pub mod surface;
```

- [ ] **Step 5: Esegui — verde**

Run: `... && make run-console-test`
Expected: `CONSOLE_TEST_PASS`.

- [ ] **Step 6: Commit**

```bash
git add kernel/src/console/surface.rs kernel/src/console/mod.rs kernel/src/console/engine_test.rs CHANGELOG/229-26-06-03-surface-backbuffer.md
git commit -m "feat(console): RAM back-buffer surface + dirty blit"
```

---

## Task 7: `render::flush` (compone celle dirty → back-buffer → blit)

**Files:**
- Create: `kernel/src/console/render.rs`
- Modify: `kernel/src/console/mod.rs`
- Modify: `kernel/src/console/engine_test.rs`

- [ ] **Step 1: Asserzione (rosso)**

In `engine_test.rs`:
```rust
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
        // Trova il primo pixel acceso della maschera di 'X' e verifica che nel
        // back-buffer non sia il colore di sfondo.
        let m = gc.mask('X', false);
        let mut found = false;
        for y in 0..gh { for x in 0..gw {
            if m.alpha[(y as usize)*(gw as usize)+(x as usize)] == 255 {
                check(20, s.read_px(x, y) == fg)?; found = true; break;
            }
        } if found { break; } }
        check(21, found)?;
        // Dopo flush la griglia è pulita.
        check(22, g.dirty_span(0).is_none())?;
    }
```

- [ ] **Step 2: Compila — fallisce**

Run: `... && make iso`
Expected: `cannot find module render`.

- [ ] **Step 3: Implementa**

`kernel/src/console/render.rs`:
```rust
//! Ponte griglia→pixel. Compone le celle dirty nel back-buffer della Surface
//! (maschera alpha × fg over bg) e blitta gli span dirty su MMIO.

use crate::console::ansi::{Cell, Rgb};
use crate::console::font::{glyph_height, glyph_width};
use crate::console::glyphcache::GlyphCache;
use crate::console::grid::Grid;
use crate::console::surface::Surface;

/// Blend per canale `fg*α + bg*(1-α)`, α = intensity/255 (come fb.rs::blend).
#[inline]
fn blend(fg: Rgb, bg: Rgb, intensity: u8) -> Rgb {
    let a = intensity as u32;
    let ia = 255 - a;
    let mix = |f: u8, b: u8| (((f as u32) * a + (b as u32) * ia) / 255) as u8;
    Rgb { r: mix(fg.r, bg.r), g: mix(fg.g, bg.g), b: mix(fg.b, bg.b) }
}

/// Compone tutte le celle dirty nel back-buffer e blitta. Azzera il dirty.
pub fn flush(grid: &mut Grid, cache: &mut GlyphCache, surf: &mut Surface) {
    let gw = glyph_width() as u32;
    let gh = glyph_height() as u32;
    for row in 0..grid.rows {
        let Some((lo, hi)) = grid.dirty_span(row) else { continue };
        for col in lo..=hi {
            compose_cell(grid.cell(col, row), col as u32, row as u32, gw, gh, cache, surf);
        }
        // Blitta lo span [lo..=hi] × righe pixel della cella in un colpo.
        let x0 = lo as u32 * gw;
        let x1 = (hi as u32 + 1) * gw - 1;
        let y0 = row as u32 * gh;
        let y1 = y0 + gh;
        surf.blit_rect(x0, x1, y0, y1);
    }
    grid.clear_dirty();
}

fn compose_cell(cell: Cell, col: u32, row: u32, gw: u32, gh: u32,
                cache: &mut GlyphCache, surf: &mut Surface) {
    let bold = cell.attr.contains(crate::console::ansi::CellAttr::BOLD);
    // Plan 1: niente rendering attributi oltre fg/bg; il bold passa alla cache
    // ma usa Regular finché il Plan 2 non aggiunge il peso.
    let mask = cache.mask(cell.ch, bold);
    let ox = col * gw;
    let oy = row * gh;
    let w = mask.w as u32;
    for ry in 0..gh {
        for rx in 0..gw {
            let alpha = if (rx < w) && ((ry as usize) < mask.h) {
                mask.alpha[(ry as usize) * mask.w + (rx as usize)]
            } else { 0 };
            let color = if alpha == 0 { cell.bg } else { blend(cell.fg, cell.bg, alpha) };
            surf.put_px(ox + rx, oy + ry, color);
        }
    }
}
```

- [ ] **Step 4: Dichiara il modulo**

In `kernel/src/console/mod.rs`:
```rust
pub mod render;
```

- [ ] **Step 5: Esegui — verde**

Run: `... && make run-console-test`
Expected: `CONSOLE_TEST_PASS`.

- [ ] **Step 6: Commit**

```bash
git add kernel/src/console/render.rs kernel/src/console/mod.rs kernel/src/console/engine_test.rs CHANGELOG/230-26-06-03-render-flush.md
git commit -m "feat(console): render flush — compose dirty cells + blit"
```

---

## Task 8: Ricostruisci `FramebufferConsole` su Grid+Surface+render

Questo è il task che cambia il comportamento runtime: il path di disegno passa da "MMIO diretto per glifo" a "Grid → flush". API pubblica (`new`, `write_str`, `clear`, `dims`, `info`) **invariata** così il resto del kernel non cambia. Il blink IRQ (`tick_cursor`) resta, leggendo `CURSOR_POS` pubblicato dalla Grid.

**Files:**
- Modify: `kernel/src/console/fb.rs`
- Modify: `kernel/src/console/engine_test.rs` (regressione cursore pubblicato)

- [ ] **Step 1: Asserzione di regressione (rosso)**

In `engine_test.rs`:
```rust
    // T23: FramebufferConsole su FbInfo finto (addr null) — write_str aggiorna
    // la griglia e pubblica il cursore.
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
```

- [ ] **Step 2: Riscrivi `fb.rs`**

Sostituisci i campi e i metodi di `FramebufferConsole`. Mantieni: `FbInfo`, `PixelLayout`, gli statics (`FB_VIRT`/`FB_PITCH`/`FB_BPP`/`CURSOR_POS`/`BLINK_COUNTER`/`BLINK_DIVIDER`), `tick_cursor()`, e `self_test()` (adattato sotto). Nuovo corpo della struct + impl:

```rust
use crate::console::grid::Grid;
use crate::console::surface::Surface;
use crate::console::glyphcache::GlyphCache;
use crate::console::render;

pub struct FramebufferConsole {
    info:   FbInfo,
    grid:   Grid,
    surf:   Surface,
    cache:  GlyphCache,
    parser: vte::Parser,
}

unsafe impl Send for FramebufferConsole {}

impl FramebufferConsole {
    pub fn new(info: FbInfo, fg: Rgb, bg: Rgb) -> Self {
        let cols = (info.width  / glyph_width()  as u32).max(1) as u16;
        let rows = (info.height / glyph_height() as u32).max(1) as u16;
        FB_VIRT.store(info.addr, Ordering::Release);
        FB_PITCH.store(info.pitch, Ordering::Release);
        FB_BPP.store(info.bpp, Ordering::Release);
        let mut me = Self {
            info,
            grid:  Grid::new(cols, rows, fg, bg),
            surf:  Surface::new(info),
            cache: GlyphCache::new(),
            parser: vte::Parser::new(),
        };
        me.clear();          // pulisce griglia + back-buffer + blit schermo
        me.publish_cursor();
        me
    }

    fn publish_cursor(&self) {
        let (c, r) = self.grid.cursor();
        let packed = ((c as u64) << 32) | (r as u64);
        CURSOR_POS.store(packed, Ordering::Release);
    }

    pub fn dims(&self) -> (u32, u32, u32, u32) {
        (self.info.width, self.info.height, self.info.pitch, self.info.bpp)
    }
    pub fn info(&self) -> FbInfo { self.info }

    #[cfg(any(test, debug_assertions))]
    pub fn cursor_for_test(&self) -> (u16, u16) { self.grid.cursor() }

    pub fn write_str(&mut self, s: &str) {
        let mut parser = core::mem::replace(&mut self.parser, vte::Parser::new());
        for b in s.bytes() {
            parser.advance(self, b);
        }
        self.parser = parser;
        render::flush(&mut self.grid, &mut self.cache, &mut self.surf);
        self.publish_cursor();
    }

    pub fn clear(&mut self) {
        self.grid.clear();
        render::flush(&mut self.grid, &mut self.cache, &mut self.surf);
        self.publish_cursor();
    }
}
```

Aggiorna l'impl `vte::Perform for FramebufferConsole` per delegare alla Grid (rimpiazza il corpo attuale):
```rust
impl vte::Perform for FramebufferConsole {
    fn print(&mut self, ch: char) { self.grid.put(ch); }
    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.grid.newline(),
            b'\r' => self.grid.cr(),
            b'\x08' => self.grid.bs(),
            b'\t' => self.grid.tab(),
            _ => {}
        }
    }
    fn csi_dispatch(&mut self, params: &vte::Params, _i: &[u8], _ignore: bool, c: char) {
        // Plan 1: porta SOLO il sottoinsieme già supportato (A/B/C/D/H/J/K/m),
        // ora mediato dalla Grid. Cursor moves + CUP non sono ancora "dirty"
        // (non disegnano), J=2 → clear, m → colori, K → erase-to-eol.
        let p1 = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(1);
        match c {
            'm' => {
                let it = params.iter().flat_map(|p| p.iter().copied());
                let (fg, bg) = apply_sgr(it, self.grid.current_colors().0, self.grid.current_colors().1);
                self.grid.set_fg(fg);
                self.grid.set_bg(bg);
            }
            'J' => {
                let arg = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(0);
                if arg == 2 { self.grid.clear(); }
            }
            // A/B/C/D/H/K: movimento cursore + erase. Aggiungi i metodi
            // corrispondenti alla Grid (goto/move/erase_to_eol) — vedi Step 3.
            'A' => self.grid.move_up(p1.max(1)),
            'B' => self.grid.move_down(p1.max(1)),
            'C' => self.grid.move_right(p1.max(1)),
            'D' => self.grid.move_left(p1.max(1)),
            'H' => {
                let mut it = params.iter();
                let row = it.next().and_then(|p| p.first().copied()).unwrap_or(1);
                let col = it.next().and_then(|p| p.first().copied()).unwrap_or(1);
                self.grid.goto(col.saturating_sub(1), row.saturating_sub(1));
            }
            'K' => self.grid.erase_to_eol(),
            _ => {}
        }
    }
    fn esc_dispatch(&mut self, _: &[u8], _: bool, _: u8) {}
    fn hook(&mut self, _: &vte::Params, _: &[u8], _: bool, _: char) {}
    fn put(&mut self, _: u8) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, _: &[&[u8]], _: bool) {}
}
```

- [ ] **Step 3: Aggiungi i metodi cursore/erase alla Grid**

In `kernel/src/console/grid.rs`, dentro `impl Grid`:
```rust
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
        self.mark(col, row);
        let hi = self.cols - 1;
        let (lo, _) = self.dirty[row as usize];
        self.dirty[row as usize] = (lo.min(col), hi);
    }
```

- [ ] **Step 4: Adatta `self_test()` al back-buffer**

`self_test()` leggeva i pixel dal FB MMIO via `draw_glyph`. Ora `draw_glyph` non esiste più. Riscrivilo per usare il path nuovo e leggere dal back-buffer della Surface. Sostituisci `fb::self_test`:
```rust
/// Boot-time self-test: scrive 'X', flush, e verifica che un pixel acceso
/// della maschera sia il colore fg nel back-buffer.
pub fn self_test(fb: &mut FramebufferConsole) -> bool {
    let fg = fb.grid.current_colors().0;
    fb.write_str("X");
    let m = fb.cache.mask('X', false);
    let gw = glyph_width(); let gh = glyph_height();
    for y in 0..gh { for x in 0..gw {
        if m.alpha[y * gw + x] == 255 {
            return fb.surf.read_px(x as u32, y as u32) == fg;
        }
    }}
    false
}
```
(Serve accesso ai campi privati `grid/cache/surf` da `self_test`, che è nello stesso modulo `fb` → ok.)

- [ ] **Step 5: Verifica i call-site rotti**

`draw_glyph`, `put_char`, i campi `cur_col/cur_row/fg/bg` erano `pub`/usati altrove? Cerca: `grep -rn "draw_glyph\|put_char\|\.cur_col\|\.cur_row" kernel/src`. Se qualcuno fuori da `fb.rs` li usa, adatta o rimuovi. `tick_cursor()` NON li usa (legge gli atomics) → invariato.

- [ ] **Step 6: Compila + esegui**

Run: `... && make run-console-test`
Expected: `CONSOLE_TEST_PASS` (incluso T23).

- [ ] **Step 7: Regressione visiva — boot reale**

Run: `... && make run-test`
Expected: passa l'asserzione di boot esistente (la stringa `HELLO`/successo del Makefile). Conferma che l'output di boot rende ancora.

- [ ] **Step 8: Commit**

```bash
git add kernel/src/console/fb.rs kernel/src/console/grid.rs kernel/src/console/engine_test.rs CHANGELOG/231-26-06-03-fb-on-grid.md
git commit -m "refactor(console): FramebufferConsole on grid + back-buffer pipeline"
```

---

## Task 9: Perf + regressione TUI (rtop/shell) + WC measure

**Files:**
- Modify: `kernel/src/console/engine_test.rs` (timing TSC)
- Modify: `kernel/src/console/surface.rs` (eventuale ottimizzazione blit)

- [ ] **Step 1: Misura TSC di un full-redraw nel self-test**

In `engine_test.rs`, aggiungi (usa l'API TSC esistente del kernel — cerca `rdtsc`/`tsc_now` nel crate, es. `crate::time::rdtsc()`):
```rust
    // T24: full-redraw 80x25 sotto soglia. Misura puramente RAM (addr null),
    // quindi misura il costo compose; il costo blit MMIO si valuta a schermo.
    {
        use crate::console::fb::{FramebufferConsole, FbInfo, PixelLayout};
        use crate::console::ansi::{WHITE, BLACK};
        use crate::console::font::{glyph_width, glyph_height};
        let gw = glyph_width() as u32; let gh = glyph_height() as u32;
        let info = FbInfo { addr: core::ptr::null_mut(), width: gw*80, height: gh*25,
                            pitch: gw*80*4, bpp: 32, pixel: PixelLayout::Bgr };
        let mut con = FramebufferConsole::new(info, WHITE, BLACK);
        let t0 = crate::time::rdtsc();
        for _ in 0..25 { con.write_str("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJ\n"); }
        let dt = crate::time::rdtsc() - t0;
        crate::serial_println!("CONSOLE_PERF: full_redraw_tsc={}", dt);
        // Soglia generosa: il compose RAM di ~2000 celle deve stare sotto ~50M cicli.
        check(24, dt < 50_000_000)?;
    }
```
Adatta `crate::time::rdtsc` al nome reale. La soglia è un guard-rail anti-regressione, non un benchmark fine.

- [ ] **Step 2: Esegui + leggi il numero**

Run: `... && make run-console-test`
Expected: `CONSOLE_TEST_PASS`; nel log compare `CONSOLE_PERF: full_redraw_tsc=<N>`. Annota N nel CHANGELOG.

- [ ] **Step 3: WC — verifica il mapping framebuffer**

Limine mappa di norma il framebuffer write-combining. Verifica: il blit reale su schermo è fluido (Step 4-5)? Se il redraw a schermo è visibilmente lento/strappato su HW reale, il FB non è WC. Documenta l'esito nel CHANGELOG. **Non** scrivere codice PAT/MTRR in questo plan se il FB è già WC (lo è quasi certamente via Limine); se NON lo è, apri un follow-up dedicato (remap WC via PAT) — fuori dallo scope del Plan 1. Misura opzionale: scrivi N pixel su FB reale in un test gated e cronometra TSC/pixel.

- [ ] **Step 4: Regressione rtop (TUI ratatui)**

Run: `... && make run-rtop-test`
Expected: passa come prima (rtop rende, marker del test verde). Conferma che una TUI full-screen funziona sul nuovo pipeline.

- [ ] **Step 5: Regressione shell + pipe + SSH**

Run, uno per uno:
- `... && make run-pipe-test`
- `... && make run-ssh-test`
- `... && make run-ctrlc-test`
Expected: tutti passano (la console è sul path di output di shell/PTY/SSH).

- [ ] **Step 6: Verifica visiva manuale**

Run: `... && make run` (QEMU con display). Osserva: boot output, prompt shell, digitazione (echo + cursore blink), `ls`, esegui `rtop` (alt-screen non ancora — rtop usa clear; ok), scroll del terminale. Niente flicker sul redraw, scroll fluido. Se hai accesso VBox, ripeti (memory: testare VBox per modifiche sensibili a timing/IRQ).

- [ ] **Step 7: CHANGELOG finale + commit**

Crea `CHANGELOG/232-26-06-03-console-backbuffer-done.md` con il numero TSC misurato e l'esito WC. Poi:
```bash
git add kernel/src/console/engine_test.rs kernel/src/console/surface.rs CHANGELOG/232-26-06-03-console-backbuffer-done.md
git commit -m "test(console): full-redraw perf guard + TUI regression; back-buffer done"
```

---

## Self-Review (eseguita)

**1. Copertura spec (Plan 1 = sezioni "veloce" + architettura):**
- Back-buffer ibrido (cell grid + pixel back-buffer): Task 4-6 ✓
- Glyph cache maschera alpha keyed (char,bold): Task 3 ✓
- Dirty blit + coalescing per riga: Task 7 (`blit_rect` su span lo..=hi) ✓
- Refactor in unità isolate (surface/grid/glyphcache/render): Task 2-8 ✓
- Write-combining: Task 9 Step 3 (verifica; PAT come follow-up se serve) ✓
- Flush policy: implementata come flush-per-write_str (vedi Nota sotto), blink IRQ invariato ✓
- Truecolor / attributi / alt-screen / cursor styles / scroll regions / scrollback / bracketed paste: **fuori Plan 1** → Plan 2 (fidelity) e Plan 3 (modern VT). `CellAttr` già definito (Task 2), cache pronta al recolor.

**2. Placeholder scan:** nessun TODO/TBD. Punti "adatta al nome reale" (macro seriale, `rdtsc`, variabili QEMU del Makefile) sono passi di verifica espliciti con come trovarli, non placeholder di contenuto.

**3. Coerenza tipi:** `Cell`/`CellAttr` (Task 2) usati identici in grid/render/fb. `GlyphMask{w,h,alpha}` (Task 3) usato in render con `mask.alpha`/`mask.w`/`mask.h`. `Grid` API (`put/cr/bs/tab/newline/scroll_up/clear/cell/cursor/dirty_span/clear_dirty/move_*/goto/erase_to_eol/set_fg/set_bg/current_colors`) coerente tra Task 4/5/8. `Surface{new/put_px/read_px/blit_rect/info}` coerente Task 6/7/8. `render::flush(grid,cache,surf)` firma identica Task 7/8.

**Deviazione dallo spec (flush policy):** lo spec prevedeva flush coalescato a ~60 Hz su tick + pre-input. Plan 1 usa **flush al termine di ogni `write_str`** perché il flush tocca Grid/Surface (dietro il `CONSOLE` Mutex) e NON può girare in IRQ senza rischio deadlock (vincolo ruos: nessun lock in IRQ; `tick_cursor` usa solo atomics). Con il dirty-tracking il flush-per-write è economico (blitta solo le celle cambiate). Il coalescing 60 Hz resta una possibile ottimizzazione futura (richiede integrazione con l'executor), da valutare solo se si osserva tearing su frame multi-write. Questa scelta è registrata qui e va riportata nel CHANGELOG del Task 9.
