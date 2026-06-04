# Terminal Engine — Plan 3: Modern VT (alt-screen + cursor + scroll regions)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Aggiungere le funzioni VT "da terminale moderno" che vivono sul path di output della console: alternate screen buffer (`?1049h/l`), stili cursore (`DECSCUSR`) + show/hide (`?25h/l`) con fix del ghost cursore, e scroll regions (`DECSTBM`).

**Architecture:** Tutto sul path `write_str → vte → Grid/FramebufferConsole`. Alt-screen = swap di due `Grid` in `FramebufferConsole`. Cursore = nuovi atomic (visible/style) letti dall'IRQ `tick_cursor`; il ghost si risolve forzando dirty la vecchia cella-cursore ad ogni flush. Scroll regions = la `Grid` limita scroll/newline alla banda `[top,bot]`.

**Tech Stack:** Rust `no_std`+`alloc`, `vte`, target `x86_64-unknown-none`. Build/test via WSL. Harness: self-test `engine_test` (T36+) dietro feature `boot-checks`, asserito da `make run-console-test`.

**Prerequisito:** Plan 1+2 merged su `main` (`c540223`). Branch `feature/terminal-engine-vt` (già creato da main).

**Fuori scope (piani separati):** scrollback buffer (Shift+PgUp — richiede intercept input keyboard/PTY + render a finestra → Plan 4); bracketed paste (`?2004` — nessuna sorgente di paste locale in ruos → rimandato).

**CHANGELOG:** una entry per task, `CHANGELOG/NN-26-06-04-<slug>.md`. Ultimi: **243** + **244** (questo piano). Parti da **245**; gli NN nei task sono indicativi — verifica con `ls CHANGELOG/ | sort -n`.

**Build/test:** `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-console-test'` → `CONSOLE_TEST_PASS`. Regressione TUI: `make run-rtop-test`.

---

## File Structure

| File | Stato | Responsabilità |
|---|---|---|
| `kernel/src/console/grid.rs` | modify | `mark_all_dirty`, `mark_cell`; scroll region (`scroll_top/bot` + `set_scroll_region`); scroll_up/newline rispettano la banda |
| `kernel/src/console/fb.rs` | modify | alt-screen swap (`saved: Option<Grid>`); ghost-fix (`last_cur`); nuovi atomic cursore (visible/style) + `tick_cursor` style-aware; csi_dispatch: `?1049`/`?25`/DECSCUSR `q`/DECSTBM `r` |
| `kernel/src/console/engine_test.rs` | modify | assert T36–T40 |

Nessun file nuovo. Niente tocca `surface`/`render`/`glyphcache`/`ansi`/`boxdraw`.

---

## Task 1: Alternate screen (`?1049h/l`) + probe private modes

**Files:** modify `grid.rs`, `fb.rs`, `engine_test.rs`.

- [ ] **Step 1 — PROBE come vte consegna i private mode `?`.** Aggiungi temporaneamente in `fb.rs` `csi_dispatch`, in cima: `crate::kprintln!("CSI i={:?} ignore={} c={}", _i, _ignore, c);` poi un test gated che fa `con.write_str("\x1b[?1049h")` e osserva il log via `make run-console-test`. Determina DOVE finisce il `?`: quasi certamente in `intermediates` (`_i == [b'?']`), con `c='h'`, params=`[1049]`. **Rimuovi il kprintln dopo.** Implementa lo Step 4 in base a quanto osservato (sotto è codificato il caso `_i` contiene `b'?'`; se vte lo consegna diversamente, adatta la condizione `is_private`).

- [ ] **Step 2 — assert (rosso)** in `engine_test.rs` prima di `Ok(())`:
```rust
    // T36: ?1049h entra in alt-screen (schermo pulito), ?1049l ripristina il primario.
    {
        use crate::console::fb::{FramebufferConsole, FbInfo, PixelLayout};
        use crate::console::ansi::{WHITE, BLACK};
        use crate::console::font::{glyph_width, glyph_height};
        let gw = glyph_width() as u32; let gh = glyph_height() as u32;
        let info = FbInfo { addr: core::ptr::null_mut(), width: gw*10, height: gh*3, pitch: gw*10*4, bpp: 32, pixel: PixelLayout::Bgr };
        let mut con = FramebufferConsole::new(info, WHITE, BLACK);
        con.write_str("PRIMARY");
        con.write_str("\x1b[?1049h");           // entra in alt
        check(36, con.cursor_for_test() == (0, 0))?; // alt parte pulita, cursore home
        con.write_str("ALT");
        con.write_str("\x1b[?1049l");           // torna al primario
        // il primario aveva "PRIMARY" → cursore era a colonna 7 riga 0
        check(37, con.cursor_for_test() == (7, 0))?;
    }
```

- [ ] **Step 3 — `grid.rs`: aggiungi `mark_all_dirty`** (`impl Grid`):
```rust
    pub fn mark_all_dirty(&mut self) {
        for d in self.dirty.iter_mut() { *d = (0, self.cols - 1); }
    }
```

- [ ] **Step 4 — `fb.rs`: alt-screen.** Aggiungi il campo `saved: Option<Grid>` alla struct (init `None` in `new`). Aggiungi i metodi e l'handler:
```rust
    fn enter_alt(&mut self) {
        if self.saved.is_none() {
            let (fg, bg) = self.grid.current_colors();
            let mut alt = Grid::new(self.grid.cols, self.grid.rows, fg, bg);
            alt.mark_all_dirty();
            self.saved = Some(core::mem::replace(&mut self.grid, alt));
        }
    }
    fn leave_alt(&mut self) {
        if let Some(mut primary) = self.saved.take() {
            primary.mark_all_dirty();
            self.grid = primary;
        }
    }
```
In `csi_dispatch`, aggiungi un ramo per `'h'`/`'l'` privati (prima/insieme agli altri rami `match c`):
```rust
            'h' | 'l' if _i.contains(&b'?') => {
                let set = c == 'h';
                for p in params.iter().flat_map(|p| p.iter().copied()) {
                    match p {
                        1049 | 1047 | 47 => if set { self.enter_alt() } else { self.leave_alt() },
                        _ => {}
                    }
                }
            }
```
(Rinomina `_i`→`i` nella firma di `csi_dispatch` per usarlo: `fn csi_dispatch(&mut self, params: &vte::Params, i: &[u8], _ignore: bool, c: char)`. Aggiorna gli altri usi se serve.)

- [ ] **Step 5 — verifica:** `make run-console-test` → `CONSOLE_TEST_PASS` (T36/T37).

- [ ] **Step 6 — commit** (`245`):
```
git add kernel/src/console/grid.rs kernel/src/console/fb.rs kernel/src/console/engine_test.rs CHANGELOG/245-26-06-04-alt-screen.md
git commit -m "feat(console): alternate screen buffer (?1049)"
```

---

## Task 2: Cursor show/hide (`?25`) + styles (`DECSCUSR`)

**Files:** modify `fb.rs`, `engine_test.rs`.

Cursore reso dall'IRQ `tick_cursor` via XOR. Aggiungiamo due atomic letti dall'IRQ: visibilità e stile (block/underline/bar). Semplificazione documentata: la distinzione steady/blink di DECSCUSR è ignorata — il cursore lampeggia sempre (comportamento terminale comune).

- [ ] **Step 1 — assert (rosso)** in `engine_test.rs`:
```rust
    // T38: ?25l nasconde, ?25h mostra; DECSCUSR imposta lo stile.
    {
        use crate::console::fb::{FramebufferConsole, FbInfo, PixelLayout, cursor_visible_for_test, cursor_style_for_test};
        use crate::console::ansi::{WHITE, BLACK};
        use crate::console::font::{glyph_width, glyph_height};
        let gw = glyph_width() as u32; let gh = glyph_height() as u32;
        let info = FbInfo { addr: core::ptr::null_mut(), width: gw*4, height: gh, pitch: gw*4*4, bpp: 32, pixel: PixelLayout::Bgr };
        let mut con = FramebufferConsole::new(info, WHITE, BLACK);
        con.write_str("\x1b[?25l");
        check(38, cursor_visible_for_test() == false)?;
        con.write_str("\x1b[?25h");
        check(39, cursor_visible_for_test() == true)?;
        con.write_str("\x1b[2 q"); // DECSCUSR 2 = block
        check(40, cursor_style_for_test() == 0)?; // 0=block
    }
```

- [ ] **Step 2 — run → FAIL** (gli atomic + getter non esistono).

- [ ] **Step 3 — `fb.rs`: aggiungi atomic + getter.** Vicino agli altri static:
```rust
use core::sync::atomic::AtomicBool;
pub(crate) static CURSOR_VISIBLE: AtomicBool = AtomicBool::new(true);
// 0=block, 1=underline, 2=bar. Default underline (comportamento Plan 1/2).
pub(crate) static CURSOR_STYLE: AtomicU32 = AtomicU32::new(1);

#[cfg(feature = "boot-checks")]
pub fn cursor_visible_for_test() -> bool { CURSOR_VISIBLE.load(Ordering::Acquire) }
#[cfg(feature = "boot-checks")]
pub fn cursor_style_for_test() -> u32 { CURSOR_STYLE.load(Ordering::Acquire) }
```

- [ ] **Step 4 — `fb.rs`: handler.** Nel ramo private `'h'|'l'` (Task 1), aggiungi il caso `25`:
```rust
                        25 => crate::console::fb::CURSOR_VISIBLE.store(set, Ordering::Release),
```
(o `CURSOR_VISIBLE.store(set, ...)` se nello stesso modulo: `CURSOR_VISIBLE.store(set, Ordering::Release)`.)
Aggiungi un ramo per DECSCUSR (`CSI n SP q` → intermediate `b' '` (0x20), action `'q'`):
```rust
            'q' if i.contains(&b' ') => {
                let n = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(1);
                let style = match n { 0 | 1 | 2 => 0u32 /*block*/, 3 | 4 => 1 /*underline*/, 5 | 6 => 2 /*bar*/, _ => 1 };
                CURSOR_STYLE.store(style, Ordering::Release);
            }
```

- [ ] **Step 5 — `fb.rs`: `tick_cursor` legge visible + style.** All'inizio, dopo il null-check, aggiungi:
```rust
    if !CURSOR_VISIBLE.load(Ordering::Acquire) { return; }
    let style = CURSOR_STYLE.load(Ordering::Acquire);
```
Sostituisci il doppio loop XOR con una regione dipendente dallo stile:
```rust
    // Regione da XOR-are in base allo stile.
    let (y0, y1, x0, x1) = match style {
        0 => (oy, oy + gh, ox, ox + gw),                 // block: tutta la cella
        2 => (oy, oy + gh, ox, ox + 2),                  // bar: 2 colonne a sinistra
        _ => (oy + gh - 2, oy + gh, ox, ox + gw),        // underline: 2 righe in fondo
    };
    for y in y0..y1 {
        for x in x0..x1 {
            let off = y * pitch + x * bpp_bytes;
            unsafe {
                let p = base.add(off);
                for k in 0..bpp_bytes {
                    let q = p.add(k);
                    let v = core::ptr::read_volatile(q);
                    core::ptr::write_volatile(q, v ^ 0xFF);
                }
            }
        }
    }
```

- [ ] **Step 6 — verifica:** `make run-console-test` → `CONSOLE_TEST_PASS`.

- [ ] **Step 7 — commit** (`246`):
```
git add kernel/src/console/fb.rs kernel/src/console/engine_test.rs CHANGELOG/246-26-06-04-cursor-style-visibility.md
git commit -m "feat(console): cursor show/hide (?25) + DECSCUSR styles"
```

---

## Task 3: Fix ghost cursore (F1)

**Files:** modify `grid.rs`, `fb.rs`, `engine_test.rs`.

Il cursore XOR sulla FB live lascia un ghost sulla vecchia cella quando si muove senza riscriverla. Fix: forza dirty la cella-cursore precedente ad ogni flush → il blit la ripulisce.

- [ ] **Step 1 — assert (rosso)** in `engine_test.rs`:
```rust
    // T41: dopo aver mosso il cursore, la vecchia cella viene marcata dirty
    // (così il flush la ripulisce dal ghost XOR).
    {
        use crate::console::fb::{FramebufferConsole, FbInfo, PixelLayout};
        use crate::console::ansi::{WHITE, BLACK};
        use crate::console::font::{glyph_width, glyph_height};
        let gw = glyph_width() as u32; let gh = glyph_height() as u32;
        let info = FbInfo { addr: core::ptr::null_mut(), width: gw*5, height: gh, pitch: gw*5*4, bpp: 32, pixel: PixelLayout::Bgr };
        let mut con = FramebufferConsole::new(info, WHITE, BLACK);
        con.write_str("AB");          // cursore a (2,0); last_cur diventa (2,0)
        con.write_str("\x1b[D");      // cursor-left → (1,0); la vecchia (2,0) va ripulita
        check(41, con.last_cur_for_test() == (1, 0))?;
    }
```

- [ ] **Step 2 — run → FAIL** (niente `last_cur_for_test`).

- [ ] **Step 3 — `grid.rs`: `mark_cell` pubblico:**
```rust
    pub fn mark_cell(&mut self, col: u16, row: u16) {
        if row < self.rows && col < self.cols { self.mark(col, row); }
    }
```

- [ ] **Step 4 — `fb.rs`: traccia `last_cur` e forza dirty.** Aggiungi campo `last_cur: (u16, u16)` (init `(0,0)` in `new`). Modifica `write_str`: prima di `render::flush`, forza dirty la vecchia cella; dopo, aggiorna `last_cur`:
```rust
    pub fn write_str(&mut self, s: &str) {
        let mut parser = core::mem::replace(&mut self.parser, vte::Parser::new());
        for b in s.bytes() { parser.advance(self, b); }
        self.parser = parser;
        // ghost-fix: ripulisci la cella dove stava il cursore (l'IRQ può averla XOR-ata)
        let (oc, or) = self.last_cur;
        self.grid.mark_cell(oc, or);
        render::flush(&mut self.grid, &mut self.cache, &mut self.surf);
        self.last_cur = self.grid.cursor();
        self.publish_cursor();
    }
```
Fai lo stesso aggiornamento di `last_cur` in `clear()` (dopo flush: `self.last_cur = self.grid.cursor();`). Aggiungi il getter di test:
```rust
    #[cfg(feature = "boot-checks")]
    pub fn last_cur_for_test(&self) -> (u16, u16) { self.last_cur }
```
Nota alt-screen: dopo `enter_alt`/`leave_alt` il cursore cambia di colpo; `last_cur` punta alla cella del buffer precedente. `mark_cell` clampa (guardie `< rows/cols`) e marca al più una cella spuria nel nuovo buffer (che verrà comunque ridisegnata da `mark_all_dirty`). Innocuo.

- [ ] **Step 5 — verifica:** `make run-console-test` → `CONSOLE_TEST_PASS`.

- [ ] **Step 6 — commit** (`247`):
```
git add kernel/src/console/grid.rs kernel/src/console/fb.rs kernel/src/console/engine_test.rs CHANGELOG/247-26-06-04-cursor-ghost-fix.md
git commit -m "fix(console): erase stale cursor cell on move (ghost fix)"
```

---

## Task 4: Scroll regions (`DECSTBM`)

**Files:** modify `grid.rs`, `fb.rs`, `engine_test.rs`.

`DECSTBM` (`CSI t;b r`) limita scroll/newline alla banda di righe `[top,bot]`. Default = intero schermo.

- [ ] **Step 1 — assert (rosso)** in `engine_test.rs`:
```rust
    // T42: con regione [0,1] su griglia 4 righe, scrivere oltre la riga 1 scrolla
    // SOLO la banda 0..=1; le righe 2,3 restano intatte.
    {
        use crate::console::grid::Grid;
        use crate::console::ansi::{WHITE, BLACK};
        let mut g = Grid::new(4, 4, WHITE, BLACK);
        g.put('R'); g.put('3'); g.goto(0,3); g.put('Z'); // riga 3 = "Z..."
        g.set_scroll_region(0, 1);
        g.goto(0,1); g.newline(); // a fondo regione → scroll banda [0,1]
        check(42, g.cell(0,3).ch == 'Z')?;            // riga fuori regione intatta
        check(43, g.cursor() == (0,1))?;              // resta sul fondo regione
    }
```

- [ ] **Step 2 — `make iso` → FAIL** (niente `set_scroll_region`).

- [ ] **Step 3 — `grid.rs`: campi regione + metodi.** Aggiungi alla struct `scroll_top: u16, scroll_bot: u16`; init in `new`: `scroll_top: 0, scroll_bot: rows - 1`. Aggiungi:
```rust
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
```
Riscrivi `scroll_up` per scrollare la banda `[scroll_top, scroll_bot]`:
```rust
    pub fn scroll_up(&mut self) {
        let cols = self.cols as usize;
        let top = self.scroll_top as usize;
        let bot = self.scroll_bot as usize;
        // Sposta le righe (top+1..=bot) in (top..=bot-1).
        let src = (top + 1) * cols;
        let end = (bot + 1) * cols;
        let dst = top * cols;
        self.cells.copy_within(src..end, dst);
        // Svuota la riga bot.
        let blank = Cell::blank(self.fg, self.bg);
        let last = bot * cols;
        for c in self.cells[last..last + cols].iter_mut() { *c = blank; }
        self.cur_row = self.scroll_bot;
        // Marca dirty la banda.
        for r in top..=bot { self.dirty[r] = (0, self.cols - 1); }
    }
```
Modifica `newline` per scrollare a fondo regione:
```rust
    pub fn newline(&mut self) {
        self.cur_col = 0;
        if self.cur_row == self.scroll_bot {
            self.scroll_up();
        } else if self.cur_row + 1 < self.rows {
            self.cur_row += 1;
        }
    }
```
Nota: `clear()` deve resettare la regione a tutto schermo (uno schermo pulito non ha regione). Aggiungi in fondo a `clear()`: `self.scroll_top = 0; self.scroll_bot = self.rows - 1;`.

- [ ] **Step 4 — `fb.rs`: handler DECSTBM** in `csi_dispatch`:
```rust
            'r' => {
                let mut it = params.iter();
                let top = it.next().and_then(|p| p.first().copied()).unwrap_or(1);
                let bot = it.next().and_then(|p| p.first().copied()).unwrap_or(self.grid.rows);
                self.grid.set_scroll_region(top.saturating_sub(1), bot.saturating_sub(1));
            }
```

- [ ] **Step 5 — verifica:** `make run-console-test` → `CONSOLE_TEST_PASS`.

- [ ] **Step 6 — commit** (`248`):
```
git add kernel/src/console/grid.rs kernel/src/console/fb.rs kernel/src/console/engine_test.rs CHANGELOG/248-26-06-04-scroll-regions.md
git commit -m "feat(console): scroll regions (DECSTBM)"
```

---

## Task 5: Integrazione + regressione

**Files:** modify `engine_test.rs`; CHANGELOG.

- [ ] **Step 1 — assert integrazione T44** in `engine_test.rs`: alt-screen preserva il primario attraverso scritture in alt.
```rust
    // T44: scritture in alt non toccano il primario salvato.
    {
        use crate::console::fb::{FramebufferConsole, FbInfo, PixelLayout};
        use crate::console::ansi::{WHITE, BLACK};
        use crate::console::font::{glyph_width, glyph_height};
        let gw = glyph_width() as u32; let gh = glyph_height() as u32;
        let info = FbInfo { addr: core::ptr::null_mut(), width: gw*10, height: gh*3, pitch: gw*10*4, bpp: 32, pixel: PixelLayout::Bgr };
        let mut con = FramebufferConsole::new(info, WHITE, BLACK);
        con.write_str("\x1b[2;1HKEEP");      // riga 2: "KEEP"
        con.write_str("\x1b[?1049h\x1b[2JALTDATA\x1b[?1049l");
        con.write_str("\x1b[3;1Hx");         // forza un flush sul primario ripristinato
        // il primario è tornato; non verifichiamo i pixel (addr null) ma che non panichi
        // e che il cursore sia coerente.
        check(44, con.cursor_for_test() == (1, 2))?; // dopo "\x1b[3;1Hx": col 1 riga 2
    }
```

- [ ] **Step 2 — `make run-console-test`** → `CONSOLE_TEST_PASS` (T1–T44).

- [ ] **Step 3 — regressione TUI:** `make run-rtop-test` → pass (rtop usa cursore + colori; non usa alt-screen ma deve restare integro). Poi `make run-test`, `make run-ssh-test` (la shell SSH potrebbe emettere `?1049`/DECSCUSR — non deve rompersi). Retry una volta se flaky QEMU.

- [ ] **Step 4 — CHANGELOG finale + commit** (`249`): registra alt-screen, cursor styles/visibility, ghost-fix, scroll regions; e i deferred (scrollback → Plan 4; bracketed paste). Steady/blink DECSCUSR semplificato a sempre-blink.
```
git add kernel/src/console/engine_test.rs CHANGELOG/249-26-06-04-vt-done.md
git commit -m "test(console): modern-VT integration + regression; Plan 3 done"
```

- [ ] **Step 5 — check visivo (UMANO):** `make run`. Verifica: esegui un'app full-screen / `rtop` → entra/esce pulito (alt-screen, niente residui); cursore nello stile/visibilità attesi; muovendo il cursore con ←/→ NIENTE ghost underline (F1 risolto); un'app che usa scroll region (es. output con header fisso) scrolla solo la banda.

---

## Self-Review

**1. Copertura (modern-VT path-console):**
- Alternate screen `?1049h/l`: Task 1 ✓ (+1047/47 alias). Probe vte private-mode incluso.
- Cursor show/hide `?25`: Task 2 ✓. DECSCUSR styles `q`: Task 2 ✓ (block/underline/bar; steady=blink semplificato).
- Ghost-fix F1: Task 3 ✓.
- Scroll regions DECSTBM `r`: Task 4 ✓.
- Deferred dichiarati: scrollback (Plan 4), bracketed paste.

**2. Placeholder scan:** nessun TODO. Lo Step 1 di Task 1 (probe vte) è un passo concreto con codice + criterio, non placeholder.

**3. Coerenza tipi/firme:**
- `Grid`: nuovi `mark_all_dirty`, `mark_cell`, `set_scroll_region`, campi `scroll_top/bot`; `scroll_up`/`newline`/`clear` aggiornati coerentemente (Task 1/3/4).
- `FramebufferConsole`: nuovi campi `saved: Option<Grid>`, `last_cur: (u16,u16)`; metodi `enter_alt`/`leave_alt`; getter test `cursor_for_test`/`last_cur_for_test`; funzioni libere `cursor_visible_for_test`/`cursor_style_for_test`.
- Atomic `CURSOR_VISIBLE`/`CURSOR_STYLE` lette da `tick_cursor` (Task 2) — `tick_cursor` resta lock-free.
- `csi_dispatch` firma: `_i`→`i` (Task 1) usato da `'h'|'l'` privati, DECSCUSR `q`, e (già) DECSTBM `r` non-privato.

**Rischi/limiti documentati:** (a) la consegna del `?` da vte va confermata col probe (Task 1 Step 1); il codice assume `i.contains(&b'?')`. (b) steady/blink DECSCUSR ridotto a sempre-blink. (c) ghost-fix copre i movimenti via `write_str` (tutti); il cursore è ancora XOR-su-FB (no re-architettura). (d) alt-screen non salva/ripristina scroll-region separatamente per buffer (accettabile; le TUI re-impostano DECSTBM all'ingresso).
