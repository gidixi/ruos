# Terminal Engine — Plan 2: Fidelity (truecolor + attributes + box-drawing)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Rendere ratatui/TUI moderne in modo corretto: truecolor SGR (`38;2`/`48;2`), attributi testo (bold/dim/underline/reverse), e glifi box-drawing procedurali — sopra il back-buffer del Plan 1.

**Architecture:** Il parsing SGR (`ansi::apply_sgr`) si estende per truecolor + attributi e ora ritorna anche `CellAttr`, salvato nella `Grid` per-cella. Il `render::compose_cell` applica gli attributi al momento del compositing (reverse = swap fg/bg, dim = scala fg, underline = riga, bold = peso font). I box-drawing (`U+2500–257F`, non coperti da Noto Size24) sono rasterizzati proceduralmente in un nuovo modulo `boxdraw`, intercettati dalla `GlyphCache` prima del font.

**Tech Stack:** Rust nightly `no_std`+`alloc`, target `x86_64-unknown-none`. Crate: `vte`, `noto-sans-mono-bitmap 0.3` (aggiunge weight Bold), `bitflags 2`. Build/test via WSL. Harness: self-test `engine_test` (T25+) dietro feature `boot-checks`, asserito da `make run-console-test`.

**Prerequisito:** Plan 1 (foundation) merged su `main` (`3aec954`). All'inizio dell'esecuzione creare un branch `feature/terminal-engine-fidelity` da `main`.

**Build/run (via WSL):**
- Test console: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-console-test'` → `CONSOLE_TEST_PASS`.
- Regressione TUI: `make run-rtop-test` (rtop usa colori+bordi+bold/reverse → esercita tutto).

**CHANGELOG:** una entry per task, `CHANGELOG/NN-26-06-04-<slug>.md`. Ultimi usati: **236** + **237** (questo piano). Parti da **238**; gli NN scritti nei task (237…242) sono indicativi (usa 238 e a salire) — verifica sempre con `ls CHANGELOG/ | sort -n` e usa il prossimo libero.

---

## File Structure

| File | Stato | Responsabilità |
|---|---|---|
| `kernel/src/console/ansi.rs` | modify | `apply_sgr` esteso: truecolor + attributi, nuova firma `(…, attr) -> (Rgb,Rgb,CellAttr)` |
| `kernel/src/console/grid.rs` | modify | aggiunge getter `current_attr()` |
| `kernel/src/console/fb.rs` | modify | handler CSI `m` passa/salva anche l'attr |
| `kernel/src/console/render.rs` | modify | `compose_cell` applica reverse/dim/underline; bold via cache |
| `kernel/src/console/font.rs` | modify | `raster_for_weight(ch,bold)` (peso Bold) |
| `kernel/src/console/glyphcache.rs` | modify | `rasterize(ch,bold)` usa il peso; intercetta box-drawing |
| `kernel/src/console/boxdraw.rs` | create | maschere procedurali `U+2500–257F` (subset ratatui) |
| `kernel/Cargo.toml` | modify | abilita il weight Bold del crate noto |
| `kernel/src/console/engine_test.rs` | modify | assert T25–T34 |
| `kernel/src/console/mod.rs` | modify | dichiara `pub mod boxdraw;` |

---

## Task 1: SGR truecolor + parsing attributi

**Files:** modify `ansi.rs`, `grid.rs`, `fb.rs`, `engine_test.rs`.

- [ ] **Step 1 — assert (rosso)** in `engine_test.rs` `run_inner()` prima di `Ok(())`:
```rust
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
```

- [ ] **Step 2 — compila → fallisce** (`make iso`): `apply_sgr` ha 3 argomenti, non 4 → errore di arità.

- [ ] **Step 3 — riscrivi `apply_sgr`** in `ansi.rs` (sostituisci la fn esistente, righe ~80-115):
```rust
/// Apply a CSI SGR parameter sequence to fg/bg/attr. Unknown params ignored.
/// 0 reset-all; 1/2/4/7 bold/dim/underline/reverse; 22/24/27 reset those;
/// 30-37/90-97 fg, 40-47/100-107 bg (16-color); 39/49 default fg/bg;
/// 38;5;N / 48;5;N indexed; 38;2;r;g;b / 48;2;r;g;b truecolor.
pub fn apply_sgr(
    mut params: impl Iterator<Item = u16>,
    mut fg: Rgb, mut bg: Rgb, mut attr: CellAttr,
) -> (Rgb, Rgb, CellAttr) {
    while let Some(p) = params.next() {
        match p {
            0 => { fg = WHITE; bg = BLACK; attr = CellAttr::empty(); }
            1 => attr.insert(CellAttr::BOLD),
            2 => attr.insert(CellAttr::DIM),
            4 => attr.insert(CellAttr::UNDERLINE),
            7 => attr.insert(CellAttr::REVERSE),
            22 => attr.remove(CellAttr::BOLD | CellAttr::DIM),
            24 => attr.remove(CellAttr::UNDERLINE),
            27 => attr.remove(CellAttr::REVERSE),
            30..=37   => fg = VGA_16[(p - 30) as usize],
            39        => fg = WHITE,
            40..=47   => bg = VGA_16[(p - 40) as usize],
            49        => bg = BLACK,
            90..=97   => fg = VGA_16[((p - 90) + 8) as usize],
            100..=107 => bg = VGA_16[((p - 100) + 8) as usize],
            38 => match params.next() {
                Some(5) => { if let Some(i) = params.next() { fg = xterm_256(i as u8); } }
                Some(2) => {
                    let r = params.next().unwrap_or(0) as u8;
                    let g = params.next().unwrap_or(0) as u8;
                    let b = params.next().unwrap_or(0) as u8;
                    fg = Rgb { r, g, b };
                }
                _ => {}
            },
            48 => match params.next() {
                Some(5) => { if let Some(i) = params.next() { bg = xterm_256(i as u8); } }
                Some(2) => {
                    let r = params.next().unwrap_or(0) as u8;
                    let g = params.next().unwrap_or(0) as u8;
                    let b = params.next().unwrap_or(0) as u8;
                    bg = Rgb { r, g, b };
                }
                _ => {}
            },
            _ => {}
        }
    }
    (fg, bg, attr)
}
```
(`CellAttr` è già definito in `ansi.rs`, stesso modulo — nessun import nuovo.)

- [ ] **Step 4 — aggiungi getter in `grid.rs`** (`impl Grid`, accanto a `current_colors`):
```rust
    pub fn current_attr(&self) -> CellAttr { self.attr }
```

- [ ] **Step 5 — aggiorna l'handler `'m'` in `fb.rs`** `csi_dispatch` (sostituisci il blocco `'m' =>`):
```rust
            'm' => {
                let it = params.iter().flat_map(|p| p.iter().copied());
                let (fg, bg, attr) = apply_sgr(
                    it,
                    self.grid.current_colors().0,
                    self.grid.current_colors().1,
                    self.grid.current_attr(),
                );
                self.grid.set_fg(fg);
                self.grid.set_bg(bg);
                self.grid.set_attr(attr);
            }
```

- [ ] **Step 6 — verifica & cerca altri call-site**: `grep -rn "apply_sgr" kernel/src` — l'unico chiamante deve essere `fb.rs`. Poi `make run-console-test` → `CONSOLE_TEST_PASS`. (Truecolor fg/bg ora rende già; gli attributi sono salvati ma non ancora resi — Task 2-4.)

- [ ] **Step 7 — CHANGELOG + commit** (`237`):
```
git add kernel/src/console/ansi.rs kernel/src/console/grid.rs kernel/src/console/fb.rs kernel/src/console/engine_test.rs CHANGELOG/237-26-06-04-sgr-truecolor-attrs.md
git commit -m "feat(console): SGR truecolor + attribute parsing"
```

---

## Task 2: render reverse + dim

**Files:** modify `render.rs`, `engine_test.rs`.

- [ ] **Step 1 — assert (rosso)** in `engine_test.rs`:
```rust
    // T29-30: reverse scambia fg/bg; dim scurisce fg.
    {
        use crate::console::grid::Grid; use crate::console::render;
        use crate::console::surface::Surface; use crate::console::glyphcache::GlyphCache;
        use crate::console::fb::{FbInfo, PixelLayout};
        use crate::console::ansi::{WHITE, BLACK, CellAttr};
        use crate::console::font::{glyph_width, glyph_height};
        let gw = glyph_width() as u32; let gh = glyph_height() as u32;
        let info = FbInfo { addr: core::ptr::null_mut(), width: gw, height: gh, pitch: gw*4, bpp: 32, pixel: PixelLayout::Bgr };
        // reverse: pixel acceso della maschera diventa bg (nero), perché fg/bg sono scambiati.
        let mut g = Grid::new(1, 1, WHITE, BLACK); let mut s = Surface::new(info); let mut gc = GlyphCache::new();
        g.set_attr(CellAttr::REVERSE); g.put('X');
        render::flush(&mut g, &mut gc, &mut s);
        let m = gc.mask('X', false);
        let mut hit = false;
        for y in 0..gh { for x in 0..gw {
            if m.alpha[(y as usize)*(gw as usize)+(x as usize)] == 255 { check(29, s.read_px(x,y) == BLACK)?; hit = true; break; }
        } if hit { break; } }
        // dim: fg pieno non è più bianco pieno (scurito verso bg).
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
```

- [ ] **Step 2 — compila/run → fallisce** (`make run-console-test`): con `compose_cell` attuale reverse non scambia → T29 FAIL (pixel resta bianco).

- [ ] **Step 3 — riscrivi `compose_cell` + aggiungi `dim`** in `render.rs`. Cambia l'import a `use crate::console::ansi::{Cell, CellAttr, Rgb};` e sostituisci `compose_cell`:
```rust
fn dim(fg: Rgb, bg: Rgb) -> Rgb { blend(fg, bg, 160) } // ~63% fg verso bg

fn compose_cell(cell: Cell, col: u32, row: u32, gw: u32, gh: u32,
                cache: &mut GlyphCache, surf: &mut Surface) {
    let bold = cell.attr.contains(CellAttr::BOLD);
    // reverse: scambia fg/bg
    let (mut fg, bg) = if cell.attr.contains(CellAttr::REVERSE) {
        (cell.bg, cell.fg)
    } else {
        (cell.fg, cell.bg)
    };
    if cell.attr.contains(CellAttr::DIM) { fg = dim(fg, bg); }
    let mask = cache.mask(cell.ch, bold);
    let ox = col * gw;
    let oy = row * gh;
    let w = mask.w as u32;
    for ry in 0..gh {
        for rx in 0..gw {
            let alpha = if (rx < w) && ((ry as usize) < mask.h) {
                mask.alpha[(ry as usize) * mask.w + (rx as usize)]
            } else { 0 };
            let color = if alpha == 0 { bg } else { blend(fg, bg, alpha) };
            surf.put_px(ox + rx, oy + ry, color);
        }
    }
}
```

- [ ] **Step 4 — verifica**: `make run-console-test` → `CONSOLE_TEST_PASS`.

- [ ] **Step 5 — commit** (`238`):
```
git add kernel/src/console/render.rs kernel/src/console/engine_test.rs CHANGELOG/238-26-06-04-render-reverse-dim.md
git commit -m "feat(console): render reverse + dim attributes"
```

---

## Task 3: render underline

**Files:** modify `render.rs`, `engine_test.rs`.

- [ ] **Step 1 — assert (rosso)** in `engine_test.rs`:
```rust
    // T31: underline disegna una riga fg sul fondo della cella.
    {
        use crate::console::grid::Grid; use crate::console::render;
        use crate::console::surface::Surface; use crate::console::glyphcache::GlyphCache;
        use crate::console::fb::{FbInfo, PixelLayout};
        use crate::console::ansi::{WHITE, BLACK, CellAttr};
        use crate::console::font::{glyph_width, glyph_height};
        let gw = glyph_width() as u32; let gh = glyph_height() as u32;
        let info = FbInfo { addr: core::ptr::null_mut(), width: gw, height: gh, pitch: gw*4, bpp: 32, pixel: PixelLayout::Bgr };
        let mut g = Grid::new(1,1,WHITE,BLACK); let mut s = Surface::new(info); let mut gc = GlyphCache::new();
        g.set_attr(CellAttr::UNDERLINE); g.put(' '); // spazio: niente glifo, solo underline
        render::flush(&mut g, &mut gc, &mut s);
        check(31, s.read_px(0, gh - 2) == WHITE && s.read_px(gw/2, gh - 2) == WHITE)?;
    }
```

- [ ] **Step 2 — run → fallisce**: lo spazio non disegna nulla → pixel a `gh-2` resta bg.

- [ ] **Step 3 — aggiungi l'underline in `compose_cell`** (in `render.rs`, DOPO il doppio loop dei pixel, prima della `}` di chiusura della fn):
```rust
    if cell.attr.contains(CellAttr::UNDERLINE) {
        let uy = oy + gh - 2;
        for rx in 0..gw { surf.put_px(ox + rx, uy, fg); }
    }
```
(`fg` qui è già l'fg effettivo dopo reverse/dim — corretto.)

- [ ] **Step 4 — verifica**: `make run-console-test` → `CONSOLE_TEST_PASS`.

- [ ] **Step 5 — commit** (`239`):
```
git add kernel/src/console/render.rs kernel/src/console/engine_test.rs CHANGELOG/239-26-06-04-render-underline.md
git commit -m "feat(console): render underline attribute"
```

---

## Task 4: bold weight (font Bold)

**Files:** modify `font.rs`, `glyphcache.rs`, `kernel/Cargo.toml`, `engine_test.rs`.

- [ ] **Step 1 — assert (rosso)** in `engine_test.rs`:
```rust
    // T32: la maschera bold differisce da quella regular per lo stesso char.
    {
        use crate::console::glyphcache::GlyphCache;
        use alloc::vec::Vec;
        let mut gc = GlyphCache::new();
        let b: Vec<u8> = gc.mask('M', true).alpha.clone();
        let r: Vec<u8> = gc.mask('M', false).alpha.clone();
        check(32, b != r)?;
    }
```

- [ ] **Step 2 — abilita il weight Bold** in `kernel/Cargo.toml` (riga noto). Cambia:
```toml
noto-sans-mono-bitmap = { version = "0.3", features = ["size_24"] }
```
in:
```toml
noto-sans-mono-bitmap = { version = "0.3", features = ["size_24", "bold"] }
```
Verifica il nome esatto della feature peso nel crate (potrebbe essere `"bold"`; se la build fallisce con "unknown feature", `grep -i bold ~/.cargo/registry/src/*/noto-sans-mono-bitmap-*/Cargo.toml` per il nome reale). Poi conferma a runtime che `get_raster('M', FontWeight::Bold, RasterHeight::Size24)` ritorna `Some` e che `get_raster_width(FontWeight::Bold, Size24) == get_raster_width(FontWeight::Regular, Size24)` (la cella deve restare della stessa larghezza). **Se** la larghezza Bold differisce da Regular (improbabile per un mono), NON usare il peso reale: fai embolden sintetico in `rasterize` (OR della maschera Regular con se stessa shiftata di 1px a destra) — vedi nota sotto.

- [ ] **Step 3 — aggiungi `raster_for_weight` in `font.rs`**:
```rust
/// Come `raster_for` ma sceglie il peso (Bold se `bold`). Fallback a '?'
/// nello stesso peso, poi a '?' Regular.
pub fn raster_for_weight(ch: char, bold: bool) -> RasterizedChar {
    let w = if bold { FontWeight::Bold } else { FontWeight::Regular };
    get_raster(ch, w, FONT_HEIGHT)
        .or_else(|| get_raster(FALLBACK, w, FONT_HEIGHT))
        .or_else(|| get_raster(FALLBACK, FontWeight::Regular, FONT_HEIGHT))
        .expect("noto fallback '?' missing")
}
```

- [ ] **Step 4 — usa il peso in `glyphcache.rs`**. Cambia `rasterize` per accettare `bold` e l'inserimento in `mask`:
```rust
    pub fn mask(&mut self, ch: char, bold: bool) -> &GlyphMask {
        self.map.entry((ch, bold)).or_insert_with(|| rasterize(ch, bold))
    }
```
e:
```rust
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
```
(Aggiorna l'`use` in cima a `glyphcache.rs`: serve `raster_for_weight`; `raster_for` resta usata da `prewarm_ascii`? No — `prewarm_ascii` chiama `self.mask(c, false)` che ora va a `rasterize(c,false)→raster_for_weight`. La vecchia `font::raster_for` potrebbe restare senza chiamanti: se diventa dead-code è solo un warning, ok; oppure rimuovila se nessuno la usa — `grep -rn raster_for kernel/src` per decidere. NON rimuovere `raster_for_weight`.)

**Nota embolden sintetico (fallback solo se Step 2 trova larghezza Bold diversa):** in `rasterize`, dopo aver riempito `alpha` in Regular, se `bold`, applica `for y in 0..h { for x in (1..w).rev() { alpha[y*w+x] = alpha[y*w+x].max(alpha[y*w+x-1]); } }` (dilata di 1px a destra). In quel caso NON cambiare `raster_for_weight`/Cargo.

- [ ] **Step 5 — verifica**: `make run-console-test` → `CONSOLE_TEST_PASS` (T32 verde, bold ≠ regular).

- [ ] **Step 6 — commit** (`240`):
```
git add kernel/Cargo.toml kernel/Cargo.lock kernel/src/console/font.rs kernel/src/console/glyphcache.rs kernel/src/console/engine_test.rs CHANGELOG/240-26-06-04-bold-weight.md
git commit -m "feat(console): bold font weight"
```

---

## Task 5: box-drawing procedurale

**Files:** create `boxdraw.rs`; modify `glyphcache.rs`, `mod.rs`, `engine_test.rs`.

- [ ] **Step 1 — assert (rosso)** in `engine_test.rs`:
```rust
    // T33-34: ─ (U+2500) ha una riga orizzontale al centro; │ (U+2502) verticale.
    {
        use crate::console::glyphcache::GlyphCache;
        use crate::console::font::{glyph_width, glyph_height};
        let gw = glyph_width(); let gh = glyph_height();
        let mut gc = GlyphCache::new();
        let hm = gc.mask('\u{2500}', false);
        let cy = gh / 2;
        let hlit = (0..gw).filter(|&x| hm.alpha[cy*gw + x] == 255).count();
        check(33, hlit >= gw / 2)?;
        let vm = gc.mask('\u{2502}', false);
        let cx = gw / 2;
        let vlit = (0..gh).filter(|&y| vm.alpha[y*gw + cx] == 255).count();
        check(34, vlit >= gh / 2)?;
    }
```

- [ ] **Step 2 — compila → fallisce** (`make iso`): nessun modulo `boxdraw` + `─` cade su Noto → '?' (niente riga centrale piena) → T33 FAIL.

- [ ] **Step 3 — crea `kernel/src/console/boxdraw.rs`**:
```rust
//! Glifi box-drawing procedurali (U+2500–257F). Noto Size24 non include il
//! blocco Box Drawing, quindi i bordi ratatui cadrebbero su '?'. Renderizziamo
//! i caratteri usati dai bordi ratatui (light/heavy/double: linee, angoli, tee,
//! croce) in una maschera alpha. Gli angoli arrotondati (╭╮╰╯) sono resi come
//! angoli light netti (arco vero = follow-up minore).

use alloc::vec;
use alloc::vec::Vec;
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
        // angoli arrotondati → resi come angoli light netti
        '\u{256D}' => a(0,1,0,1), '\u{256E}' => a(0,1,1,0),
        '\u{256F}' => a(1,0,1,0), '\u{2570}' => a(1,0,0,1),
        _ => None,
    }
}

#[inline]
fn put(a: &mut [u8], w: usize, h: usize, x: usize, y: usize) {
    if x < w && y < h { a[y * w + x] = 255; }
}

// Braccio orizzontale: riga(e) al centro verticale, span [x0,x1).
fn harm(a: &mut [u8], w: usize, h: usize, cy: usize, x0: usize, x1: usize, weight: u8) {
    let rows: &[i32] = match weight { 1 => &[0], 2 => &[-1,0,1], 3 => &[-2,2], _ => &[] };
    for &dy in rows {
        let y = cy as i32 + dy;
        if y < 0 { continue; }
        for x in x0..x1 { put(a, w, h, x, y as usize); }
    }
}

// Braccio verticale: colonna(e) al centro orizzontale, span [y0,y1).
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
    // bracci dal centro al bordo (includono il centro per connettere gli angoli)
    harm(&mut alpha, w, h, cy, 0, cx + 1, d.left);
    harm(&mut alpha, w, h, cy, cx, w, d.right);
    varm(&mut alpha, w, h, cx, 0, cy + 1, d.up);
    varm(&mut alpha, w, h, cx, cy, h, d.down);
    Some(GlyphMask { w, h, alpha })
}
```

- [ ] **Step 4 — dichiara il modulo** in `kernel/src/console/mod.rs`: `pub mod boxdraw;`.

- [ ] **Step 5 — intercetta in `glyphcache.rs::rasterize`** (prima del font, all'inizio di `rasterize`):
```rust
fn rasterize(ch: char, bold: bool) -> GlyphMask {
    if let Some(m) = crate::console::boxdraw::mask(ch) {
        return m;
    }
    // ... (resto invariato: peso font)
```
(I box-char ignorano `bold` — linee uguali.)

- [ ] **Step 6 — verifica**: `make run-console-test` → `CONSOLE_TEST_PASS` (T33/T34 verdi).

- [ ] **Step 7 — commit** (`241`):
```
git add kernel/src/console/boxdraw.rs kernel/src/console/glyphcache.rs kernel/src/console/mod.rs kernel/src/console/engine_test.rs CHANGELOG/241-26-06-04-boxdraw.md
git commit -m "feat(console): procedural box-drawing glyphs"
```

---

## Task 6: integrazione + regressione TUI

**Files:** modify `engine_test.rs`; CHANGELOG.

- [ ] **Step 1 — assert d'integrazione (T35)** in `engine_test.rs`: una sequenza SGR completa via `FramebufferConsole` aggiorna fg/bg/attr coerentemente.
```rust
    // T35: SGR truecolor+bold+underline applicati via il path vte completo.
    {
        use crate::console::fb::{FramebufferConsole, FbInfo, PixelLayout};
        use crate::console::ansi::{WHITE, BLACK};
        use crate::console::font::{glyph_width, glyph_height};
        let gw = glyph_width() as u32; let gh = glyph_height() as u32;
        let info = FbInfo { addr: core::ptr::null_mut(), width: gw*10, height: gh, pitch: gw*10*4, bpp: 32, pixel: PixelLayout::Bgr };
        let mut con = FramebufferConsole::new(info, WHITE, BLACK);
        // ESC[1;4;38;2;200;100;50m  → bold+underline+fg truecolor
        con.write_str("\x1b[1;4;38;2;200;100;50mA\x1b[0mB");
        // cursore avanzato di 2 (A, B); reset non rompe nulla.
        check(35, con.cursor_for_test() == (2, 0))?;
    }
```

- [ ] **Step 2 — run** `make run-console-test` → `CONSOLE_TEST_PASS` (T1–T35).

- [ ] **Step 3 — regressione TUI**: `make run-rtop-test` → deve passare (rtop usa colori, bordi box-drawing, bold/reverse; ora resi correttamente). Se fallisce, indaga: regressione reale del rendering o flakiness QEMU (riprova una volta).

- [ ] **Step 4 — regressione base**: `make run-test` → success marker.

- [ ] **Step 5 — CHANGELOG finale + commit** (`242`): registra cosa è reso ora (truecolor, bold/dim/underline/reverse, box-drawing subset) e i follow-up (angoli arrotondati netti non arcuati; box-char fuori dal subset → '?').
```
git add kernel/src/console/engine_test.rs CHANGELOG/242-26-06-04-fidelity-done.md
git commit -m "test(console): SGR/box-drawing integration + TUI regression; fidelity done"
```

- [ ] **Step 6 — check visivo (UMANO)**: non automatizzabile. Dopo i task, chiedere all'utente di lanciare `make run` e verificare: `rtop` con bordi netti + colori reali (truecolor), testo bold più spesso, reverse (barre selezione), underline. Verificare niente regressioni su shell/scroll.

---

## Self-Review

**1. Copertura spec (fidelity MUST):**
- Truecolor `38;2`/`48;2`: Task 1 ✓ (parsing) + reso via `compose_cell` che usa `cell.fg/bg` ✓
- Attributi bold/dim/underline/reverse: parsing Task 1 ✓; render reverse/dim Task 2 ✓, underline Task 3 ✓, bold Task 4 ✓
- Box-drawing `U+2500–257F`: Task 5 ✓ (subset ratatui: light/heavy/double, angoli/tee/croce; arrotondati resi netti)
- Default fg/bg (39/49) e reset (0/22/24/27): Task 1 ✓

**2. Placeholder scan:** nessun TODO/TBD. I punti "verifica il nome feature"/"se larghezza Bold differisce → embolden" sono branch concreti con codice esatto fornito, non placeholder.

**3. Coerenza tipi/firme:**
- `apply_sgr(params, fg, bg, attr) -> (Rgb,Rgb,CellAttr)` (Task 1) usato identico in `fb.rs` (Task 1 Step 5) e nei test.
- `Grid::current_attr() -> CellAttr` (Task 1) usato in `fb.rs`.
- `compose_cell` reverse/dim (Task 2) + underline (Task 3) condividono la `fg` effettiva.
- `glyphcache::rasterize(ch, bold)` (Task 4) + intercetta `boxdraw::mask(ch)` (Task 5) coerenti; `mask(ch,bool)` firma invariata verso i chiamanti (render/prewarm).
- `boxdraw::mask(ch) -> Option<GlyphMask>` (Task 5) ritorna lo stesso `GlyphMask{w,h,alpha}` di `glyphcache`.
- `font::raster_for_weight(ch,bool)` (Task 4) unica nuova fn font; `raster_for` resta per compat (o rimossa se dead).

**Deviazioni/limiti documentati:** angoli arrotondati resi come angoli light netti (non arcuati) — follow-up minore. Box-char fuori dal subset ratatui → fallback '?' (raro). Bold ASCII non pre-scaldato → prima cella bold alloca una volta (panic path resta ASCII-Regular, già pre-scaldato).
