# Rust Framebuffer Console Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Noto Sans Mono framebuffer console driven by Limine's `FramebufferRequest`, abstract logging behind a `Console` trait + `MultiConsole` fan-out, refactor `kprintln!` to use it, wire ANSI escape parsing (`vte`), and blink the cursor from the LAPIC timer IRQ.

**Architecture:** Four layered tasks. Task 1 stands up the framebuffer + font + pixel rendering + a boot-time self-test (still logging through the existing serial). Task 2 adds the `Console` trait, `MultiConsole`, `SerialConsole`, a global `CONSOLE` static, and reroutes `kprintln!` to it; behavior is identical because the framebuffer is not attached yet. Task 3 attaches the `FramebufferConsole` into `MultiConsole` at boot so every print from then on shows on screen. Task 4 routes bytes through `vte::Parser` for ANSI escapes and hooks the timer ISR to toggle the cursor at ~4 Hz directly via atomics — no lock held in interrupt context.

**Tech Stack:** Rust nightly `nightly-2026-05-26`, existing `spin`/`alloc`/`x86_64`/`limine`/`talc`/`uart_16550`/`acpi`/`bitflags`. New deps: `noto-sans-mono-bitmap = "0.3"` (Task 1), `vte = "0.13"` (Task 4). WSL Ubuntu host.

---

## Key facts

- All build/run via **WSL Ubuntu** as root, cargo env sourced:
  ```
  wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'
  ```
  Edit on Windows paths. Git in normal shell. Branch `feature/fb-console`. Do not push, do not skip hooks.
- **Spec:** `docs/superpowers/specs/2026-05-28-rust-fb-console-design.md`.
- Limine framebuffer is exposed by adding a `FramebufferRequest` static in `main.rs`, bracketed by the existing `RequestsStartMarker`/`RequestsEndMarker`. Limine fills `response.framebuffers().first()` with `address` (already HHDM-mapped), `width`, `height`, `pitch`, `bpp`, `memory_model`, and the `r_/g_/b_mask_size`/`mask_shift` fields.
- Limine's framebuffer pointer is already a virtual HHDM-mapped address — we do NOT call `memory::map_io_page` for it (unlike LAPIC/IOAPIC MMIO which Limine doesn't HHDM-map). Just write directly via the returned pointer.
- TEST_PASS Makefile assert stays `ruos: ticks=`. New observable lines:
  - Task 1: `ruos: fb ok WxH pitch=P bpp=B`, `ruos: fb test ok`.
  - Task 2: no new lines (serial-only behavior preserved).
  - Task 3: no new lines on serial (visual: boot log appears on screen).
  - Task 4: `ruos: ansi test ok` and visible red "ERR" + blinking cursor on screen.

## File structure (target after Task 4)

```
kernel/src/console/
  mod.rs          # Console trait + MultiConsole + global CONSOLE
  serial_con.rs   # SerialConsole impl Console
  font.rs         # raster_for(ch), GLYPH_WIDTH/HEIGHT helpers
  ansi.rs         # Rgb, VGA_16 palette, xterm_256, apply_sgr
  fb_init.rs      # Limine FramebufferRequest + init() returning FramebufferConsole
  fb.rs           # FramebufferConsole + vte::Perform impl + tick_cursor()
kernel/src/main.rs                  # FRAMEBUFFER_REQUEST static + kmain wiring
kernel/src/timer.rs                 # extends timer_handler to call console::fb::tick_cursor
kernel/src/kprint.rs                # macro retargeted to CONSOLE
```

---

## Task 1: Framebuffer low-level (request + font + render + self-test)

**Files:**
- Modify: `kernel/Cargo.toml`
- Create: `kernel/src/console/mod.rs`
- Create: `kernel/src/console/font.rs`
- Create: `kernel/src/console/ansi.rs`
- Create: `kernel/src/console/fb_init.rs`
- Create: `kernel/src/console/fb.rs`
- Modify: `kernel/src/main.rs`
- Create: `CHANGELOG/47-26-05-28-fb-lowlevel.md`

- [ ] **Step 1: Add `noto-sans-mono-bitmap` to deps**

In `kernel/Cargo.toml` `[dependencies]`:
```toml
noto-sans-mono-bitmap = "0.3"
```
(If `0.3` does not resolve, use the latest published 0.x and note the version.)

- [ ] **Step 2: Create `kernel/src/console/ansi.rs`** (minimal — colors only; SGR parser arrives in Task 4)
```rust
//! Color types and the basic 16-color VGA palette. SGR parsing is added in
//! Task 4 (ANSI escapes); for Task 1 we only need WHITE/BLACK.

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

pub const WHITE: Rgb = Rgb { r: 0xEE, g: 0xEE, b: 0xEE };
pub const BLACK: Rgb = Rgb { r: 0x00, g: 0x00, b: 0x00 };
```

- [ ] **Step 3: Create `kernel/src/console/font.rs`**
```rust
//! Noto Sans Mono Size 16 lookup wrapper. Falls back to '?' on missing glyphs.

use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight, RasterizedChar};

pub const FONT_HEIGHT: RasterHeight = RasterHeight::Size16;
pub const FONT_WEIGHT: FontWeight   = FontWeight::Regular;

/// Glyph cell dimensions. Width is constant for a given (weight, height)
/// in noto-sans-mono-bitmap 0.3.
pub fn glyph_width() -> usize {
    get_raster_width(FONT_WEIGHT, FONT_HEIGHT)
}

pub const fn glyph_height() -> usize {
    FONT_HEIGHT.val()
}

const FALLBACK: char = '?';

pub fn raster_for(ch: char) -> RasterizedChar {
    get_raster(ch, FONT_WEIGHT, FONT_HEIGHT)
        .unwrap_or_else(|| get_raster(FALLBACK, FONT_WEIGHT, FONT_HEIGHT)
            .expect("noto fallback '?' missing"))
}
```

(If `get_raster_width` is `const fn` in the resolved crate version, you may
elevate `glyph_width()` to `const fn` accordingly; otherwise leave as a
plain `fn` that reads it once per call.)

- [ ] **Step 4: Create `kernel/src/console/fb_init.rs`**
```rust
//! Limine FramebufferRequest + init constructing a FramebufferConsole.

use crate::console::ansi::{BLACK, WHITE};
use crate::console::fb::{FbInfo, FramebufferConsole, PixelLayout};

#[derive(Debug)]
pub enum FbInitError {
    NoResponse,
    NoFramebuffer,
    UnsupportedBpp,
}

impl core::fmt::Display for FbInitError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FbInitError::NoResponse     => f.write_str("no response"),
            FbInitError::NoFramebuffer  => f.write_str("no framebuffer"),
            FbInitError::UnsupportedBpp => f.write_str("unsupported bpp"),
        }
    }
}

pub fn init() -> Result<FramebufferConsole, FbInitError> {
    let resp = crate::FRAMEBUFFER_REQUEST.response().ok_or(FbInitError::NoResponse)?;
    let fb = resp.framebuffers().next().ok_or(FbInitError::NoFramebuffer)?;

    if fb.bpp() != 32 && fb.bpp() != 24 {
        return Err(FbInitError::UnsupportedBpp);
    }

    // Detect pixel layout from RGB mask shifts. Most QEMU/VBox configs land on
    // BGR (blue at shift 0). Override with Rgb if mask says so.
    let pixel = if fb.red_mask_shift() == 0 && fb.blue_mask_shift() == 16 {
        PixelLayout::Rgb
    } else {
        PixelLayout::Bgr
    };

    let info = FbInfo {
        addr:   fb.addr() as *mut u8,
        width:  fb.width()  as u32,
        height: fb.height() as u32,
        pitch:  fb.pitch()  as u32,
        bpp:    fb.bpp()    as u32,
        pixel,
    };

    Ok(FramebufferConsole::new(info, WHITE, BLACK))
}
```

(Limine 0.6.3 exposes the framebuffer entry via `resp.framebuffers()` returning an iterator; field accessor names like `.addr()`, `.bpp()`, `.red_mask_shift()` may differ across patch versions. Adapt minimally to whatever resolves.)

- [ ] **Step 5: Create `kernel/src/console/fb.rs`** (low-level rendering; vte + cursor blink added in Task 4)
```rust
//! Framebuffer console: text rendering on Limine's framebuffer.
//!
//! Task 1: printable ASCII, '\n', '\r', '\b', scrolling, clear. No ANSI
//! parsing, no cursor blink. Task 4 adds vte::Parser + cursor blink.

use core::ptr::write_volatile;
use crate::console::ansi::{Rgb, BLACK};
use crate::console::font::{glyph_height, glyph_width, raster_for};

#[derive(Debug, Copy, Clone)]
pub enum PixelLayout { Rgb, Bgr }

#[derive(Debug, Copy, Clone)]
pub struct FbInfo {
    pub addr:   *mut u8,
    pub width:  u32,
    pub height: u32,
    pub pitch:  u32,
    pub bpp:    u32,
    pub pixel:  PixelLayout,
}

pub struct FramebufferConsole {
    info:     FbInfo,
    cols:     u32,
    rows:     u32,
    cur_col:  u32,
    cur_row:  u32,
    fg:       Rgb,
    bg:       Rgb,
}

unsafe impl Send for FramebufferConsole {}

impl FramebufferConsole {
    pub fn new(info: FbInfo, fg: Rgb, bg: Rgb) -> Self {
        let cols = (info.width  / glyph_width()  as u32).max(1);
        let rows = (info.height / glyph_height() as u32).max(1);
        let mut me = Self { info, cols, rows, cur_col: 0, cur_row: 0, fg, bg };
        me.clear();
        me
    }

    pub fn dims(&self) -> (u32, u32, u32, u32) {
        (self.info.width, self.info.height, self.info.pitch, self.info.bpp)
    }

    pub fn info(&self) -> FbInfo { self.info }

    pub fn write_str(&mut self, s: &str) {
        for ch in s.chars() { self.put_char(ch); }
    }

    pub fn put_char(&mut self, ch: char) {
        match ch {
            '\n' => self.newline(),
            '\r' => { self.cur_col = 0; }
            '\x08' => { if self.cur_col > 0 { self.cur_col -= 1; } }
            '\t' => { self.cur_col = (self.cur_col + 8) & !7; if self.cur_col >= self.cols { self.newline(); } }
            _ => {
                if self.cur_col >= self.cols { self.newline(); }
                self.draw_glyph(self.cur_col, self.cur_row, ch, self.fg, self.bg);
                self.cur_col += 1;
            }
        }
    }

    pub fn clear(&mut self) {
        for y in 0..self.info.height {
            for x in 0..self.info.width {
                self.pixel_write(x, y, self.bg);
            }
        }
        self.cur_col = 0;
        self.cur_row = 0;
    }

    fn newline(&mut self) {
        self.cur_col = 0;
        self.cur_row += 1;
        if self.cur_row >= self.rows {
            self.scroll_up();
            self.cur_row = self.rows - 1;
        }
    }

    fn scroll_up(&mut self) {
        let gh   = glyph_height() as u32;
        let pitch = self.info.pitch as usize;
        let src_row = gh as usize;
        let dst_rows = (self.info.height - gh) as usize;
        // SAFETY: src and dst lie within the framebuffer mapping, sizes within bounds.
        unsafe {
            let base = self.info.addr;
            for y in 0..dst_rows {
                let src = base.add((y + src_row) * pitch);
                let dst = base.add(y * pitch);
                core::ptr::copy(src, dst, pitch);
            }
        }
        // Clear the bottom band.
        for y in (self.info.height - gh)..self.info.height {
            for x in 0..self.info.width {
                self.pixel_write(x, y, self.bg);
            }
        }
    }

    pub fn draw_glyph(&self, col: u32, row: u32, ch: char, fg: Rgb, bg: Rgb) {
        let raster = raster_for(ch);
        let gw = glyph_width() as u32;
        let gh = glyph_height() as u32;
        let ox = col * gw;
        let oy = row * gh;
        for (ry, line) in raster.raster().iter().enumerate() {
            for (rx, intensity) in line.iter().enumerate() {
                let color = if *intensity >= 128 { fg } else { bg };
                self.pixel_write(ox + rx as u32, oy + ry as u32, color);
            }
        }
    }

    fn pixel_write(&self, x: u32, y: u32, c: Rgb) {
        if x >= self.info.width || y >= self.info.height { return; }
        let off = (y as usize) * (self.info.pitch as usize)
                + (x as usize) * (self.info.bpp as usize / 8);
        // SAFETY: off is within the framebuffer; bpp is 24 or 32 (checked at init).
        unsafe {
            let p = self.info.addr.add(off);
            let (b0, b1, b2) = match self.info.pixel {
                PixelLayout::Bgr => (c.b, c.g, c.r),
                PixelLayout::Rgb => (c.r, c.g, c.b),
            };
            write_volatile(p.add(0), b0);
            write_volatile(p.add(1), b1);
            write_volatile(p.add(2), b2);
            if self.info.bpp == 32 {
                write_volatile(p.add(3), 0u8);
            }
        }
    }
}

/// Boot-time self-test: render 'X' at (0,0), read back the glyph rectangle,
/// compare each pixel to the expected color according to the Noto raster
/// (intensity >= 128 → fg, else bg). Returns true on full match.
pub fn self_test(fb: &mut FramebufferConsole) -> bool {
    fb.draw_glyph(0, 0, 'X', fb.fg, fb.bg);

    let raster = raster_for('X');
    let bpp_bytes = (fb.info.bpp as usize) / 8;
    let pitch = fb.info.pitch as usize;
    let (fg, bg) = (fb.fg, fb.bg);

    for (ry, line) in raster.raster().iter().enumerate() {
        for (rx, intensity) in line.iter().enumerate() {
            let expect = if *intensity >= 128 { fg } else { bg };
            let off = ry * pitch + rx * bpp_bytes;
            // SAFETY: bounds checked by draw_glyph above.
            let (b0, b1, b2) = unsafe {
                let p = fb.info.addr.add(off);
                (
                    core::ptr::read_volatile(p.add(0)),
                    core::ptr::read_volatile(p.add(1)),
                    core::ptr::read_volatile(p.add(2)),
                )
            };
            let (eb0, eb1, eb2) = match fb.info.pixel {
                PixelLayout::Bgr => (expect.b, expect.g, expect.r),
                PixelLayout::Rgb => (expect.r, expect.g, expect.b),
            };
            if (b0, b1, b2) != (eb0, eb1, eb2) {
                return false;
            }
        }
    }
    true
}
```

- [ ] **Step 6: Create `kernel/src/console/mod.rs`** (skeleton; Console trait + MultiConsole arrive in Task 2)
```rust
//! Console subsystem: framebuffer rendering, ANSI parsing, fan-out logging.
//! Task 1 only ships the framebuffer rendering primitives + self-test.

pub mod ansi;
pub mod font;
pub mod fb;
pub mod fb_init;
```

- [ ] **Step 7: Add Limine `FramebufferRequest` static in `kernel/src/main.rs`**

Add to the imports near the other limine request types:
```rust
use limine::request::FramebufferRequest;
```
After the existing `HHDM_REQUEST` (or `RSDP_REQUEST`) static block:
```rust
#[used]
#[link_section = ".requests"]
pub static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();
```
Add `mod console;` next to the other `mod` declarations.

- [ ] **Step 8: Wire the self-test in `kmain`**

In `kernel/src/main.rs`, AFTER the existing `ruos: vfs smoke ok ...` log block and BEFORE the `while timer::ticks() < 10` busy-wait, add:
```rust
    let _fb_keep = match console::fb_init::init() {
        Ok(mut fb) => {
            let (w, h, p, b) = fb.dims();
            kprintln!("ruos: fb ok {}x{} pitch={} bpp={}", w, h, p, b);
            let ok = console::fb::self_test(&mut fb);
            kprintln!("ruos: fb test {}", if ok { "ok" } else { "fail" });
            Some(fb)
        }
        Err(e) => {
            kprintln!("ruos: fb fail: {}", e);
            None
        }
    };
    // `_fb_keep` is dropped at end of kmain; Task 3 stashes it in CONSOLE
    // so the framebuffer stays attached for subsequent prints.
    core::mem::forget(_fb_keep); // do not call FramebufferConsole drop on shutdown path
```

(The `core::mem::forget` line is defensive: `FramebufferConsole` has no Drop impl that would run, but we don't want a future Drop to do anything spooky to the live framebuffer.)

- [ ] **Step 9: Build and run**
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -15'
```
Expected serial includes the two new lines plus `TEST_PASS`:
```
ruos: vfs smoke ok n=3 buf=[abc]
ruos: fb ok 1024x768 pitch=4096 bpp=32   (or similar)
ruos: fb test ok
ruos: ticks=N
```
If `fb fail: no response` appears: the Limine config or QEMU framebuffer isn't being provided; the kernel continues. If `fb test fail`: the rendered pixels do not match the Noto raster — investigate the `PixelLayout` choice in `fb_init` first (some firmwares report RGB shifts opposite to BGR).

- [ ] **Step 10: Changelog**
Create `CHANGELOG/47-26-05-28-fb-lowlevel.md`:
```markdown
# 47 — Framebuffer low-level (Limine FB + Noto + self-test)

**Data:** 2026-05-28

## Cosa
- Aggiunta dep `noto-sans-mono-bitmap = "0.3"`.
- Nuovo modulo `kernel/src/console/` con:
  - `ansi.rs` — Rgb + WHITE/BLACK (palette completa arriva in Task 4).
  - `font.rs` — wrappers Noto Size 16 con fallback '?'.
  - `fb_init.rs` — Limine FramebufferRequest + `init()` ritorna
    `FramebufferConsole` o `FbInitError`.
  - `fb.rs` — `FramebufferConsole` con `new/clear/put_char/scroll_up/
    draw_glyph/pixel_write` (ASCII printable + \n/\r/\b/\t, scroll, no
    ANSI, no blink). `self_test('X')` legge pixel rendered e li confronta
    al raster atteso.
- `main.rs` — `FRAMEBUFFER_REQUEST` static + `mod console;` + smoke test
  al boot: log `fb ok WxH pitch=P bpp=B` + `fb test ok`/`fb test fail`.
  `FramebufferConsole` viene `mem::forget`ato (Task 3 lo attaccherà a
  MultiConsole).

## Perché
Primo pezzo dello Step 8: rendering framebuffer funzionante e verificabile
prima di toccare `kprintln!` e la fan-out infrastructure.

## File toccati
- kernel/Cargo.toml, kernel/Cargo.lock
- kernel/src/console/* (nuovi)
- kernel/src/main.rs
- CHANGELOG/47-26-05-28-fb-lowlevel.md
```

- [ ] **Step 11: Commit**
```bash
git add kernel/Cargo.toml kernel/Cargo.lock kernel/src/console kernel/src/main.rs \
        CHANGELOG/47-26-05-28-fb-lowlevel.md
git commit -m "feat(rust): framebuffer console low-level + Noto + self-test"
```

---

## Task 2: Console trait + MultiConsole + SerialConsole + kprintln refactor

**Files:**
- Modify: `kernel/src/console/mod.rs`
- Create: `kernel/src/console/serial_con.rs`
- Modify: `kernel/src/kprint.rs`
- Create: `CHANGELOG/48-26-05-28-console-trait.md`

- [ ] **Step 1: Create `kernel/src/console/serial_con.rs`**
```rust
//! Console impl that delegates to the existing SERIAL global. ANSI escape
//! bytes are sent raw so host terminal emulators (or SSH clients) render
//! them on the other end.

use core::fmt::Write as _;

pub struct SerialConsole;

impl crate::console::Console for SerialConsole {
    fn write_str(&mut self, s: &str) {
        let _ = crate::serial::SERIAL.lock().write_str(s);
    }
    fn clear(&mut self) { /* no-op on serial */ }
}
```

- [ ] **Step 2: Add Console trait + MultiConsole + CONSOLE to `kernel/src/console/mod.rs`**

Replace the current contents of `kernel/src/console/mod.rs` with:
```rust
//! Console subsystem: framebuffer rendering, ANSI parsing, fan-out logging.

pub mod ansi;
pub mod font;
pub mod fb;
pub mod fb_init;
pub mod serial_con;

use core::fmt;
use spin::Mutex;
use crate::console::fb::FramebufferConsole;
use crate::console::serial_con::SerialConsole;

pub trait Console {
    fn write_str(&mut self, s: &str);
    fn clear(&mut self);
}

pub struct MultiConsole {
    pub serial: SerialConsole,
    pub fb:     Option<FramebufferConsole>,
}

impl MultiConsole {
    pub const fn new() -> Self {
        Self { serial: SerialConsole, fb: None }
    }

    /// Stash a constructed FramebufferConsole. From now on every write_str
    /// also reaches the framebuffer. Called by Task 3 wiring in kmain.
    pub fn attach_framebuffer(&mut self, fb: FramebufferConsole) {
        self.fb = Some(fb);
    }
}

impl fmt::Write for MultiConsole {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        Console::write_str(&mut self.serial, s);
        if let Some(fb) = &mut self.fb {
            Console::write_str(fb, s);
        }
        Ok(())
    }
}

// FramebufferConsole + SerialConsole both expose `pub fn write_str(&mut self, &str)`
// as inherent methods (FramebufferConsole) or trait impls (SerialConsole). For
// the fb side we need a `Console` impl too so the `if let Some(fb)` arm above
// compiles uniformly. Provide it here:
impl Console for FramebufferConsole {
    fn write_str(&mut self, s: &str) { FramebufferConsole::write_str(self, s); }
    fn clear(&mut self)              { FramebufferConsole::clear(self); }
}

pub static CONSOLE: Mutex<MultiConsole> = Mutex::new(MultiConsole::new());
```

- [ ] **Step 3: Retarget `kernel/src/kprint.rs` to CONSOLE**

Replace the body of the `kprintln!` macro:
```rust
//! `kprintln!` macro built on the global multi-console writer.
//!
//! The body runs with interrupts disabled so an interrupt handler that also
//! calls `kprintln!` (e.g. the timer or keyboard ISR) cannot deadlock
//! against a preempted holder of the CONSOLE spin lock.

#[macro_export]
macro_rules! kprintln {
    ($($arg:tt)*) => {{
        use core::fmt::Write as _;
        ::x86_64::instructions::interrupts::without_interrupts(|| {
            let _ = writeln!($crate::console::CONSOLE.lock(), $($arg)*);
        });
    }};
}
```

(The previous version targeted `$crate::serial::SERIAL.lock()`. Behavior is
identical at this stage because the `fb` arm of MultiConsole is `None`.)

- [ ] **Step 4: Build and run**
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -15'
```
Expected: serial output is **byte-for-byte identical** to Task 1's. The
framebuffer line `fb ok ... / fb test ok` still appears (it goes through
`kprintln!` → CONSOLE → SerialConsole). `TEST_PASS`.

If a regression appears, the most likely cause is the SerialConsole loop
using `from_utf8(&[byte])` failing for multi-byte UTF-8 sequences. The
fallback to `"?"` is intentional and matches the existing
`devices::ConsoleFile::write` pattern (Step 7).

- [ ] **Step 5: Changelog**
Create `CHANGELOG/48-26-05-28-console-trait.md`:
```markdown
# 48 — Console trait + MultiConsole + kprintln refactor (Step 8 Task 2)

**Data:** 2026-05-28

## Cosa
- `kernel/src/console/mod.rs` (riscritto): trait `Console`, struct
  `MultiConsole { serial, fb: Option<_> }`, impl `fmt::Write` per fan-out,
  static globale `CONSOLE: spin::Mutex<MultiConsole>` const-construttibile.
- `kernel/src/console/serial_con.rs` (nuovo): `SerialConsole` forwarder
  byte-per-byte alla `SERIAL` esistente.
- `kernel/src/kprint.rs`: `kprintln!` ora scrive su `CONSOLE.lock()` invece
  che su `SERIAL.lock()`. Behavior immutato finché FramebufferConsole non
  è attaccato (Task 3).
- `FramebufferConsole` ha ora un `impl Console` che delega ai metodi
  inerenti già esistenti.

## Perché
Secondo pezzo dello Step 8: abstraction layer per fan-out logging senza
ancora coinvolgere il framebuffer attivo.

## File toccati
- kernel/src/console/mod.rs
- kernel/src/console/serial_con.rs (nuovo)
- kernel/src/kprint.rs
- CHANGELOG/48-26-05-28-console-trait.md
```

- [ ] **Step 6: Commit**
```bash
git add kernel/src/console/mod.rs kernel/src/console/serial_con.rs kernel/src/kprint.rs \
        CHANGELOG/48-26-05-28-console-trait.md
git commit -m "feat(rust): Console trait + MultiConsole + kprintln retargeted"
```

---

## Task 3: Attach FramebufferConsole to MultiConsole at boot

**Files:**
- Modify: `kernel/src/main.rs`
- Create: `CHANGELOG/49-26-05-28-fb-attach.md`

- [ ] **Step 1: Replace the Task 1 self-test block in `kmain`**

In `kernel/src/main.rs`, replace the block introduced in Task 1 Step 8:
```rust
    let _fb_keep = match console::fb_init::init() {
        ...
        core::mem::forget(_fb_keep);
```
with:
```rust
    match console::fb_init::init() {
        Ok(mut fb) => {
            let (w, h, p, b) = fb.dims();
            kprintln!("ruos: fb ok {}x{} pitch={} bpp={}", w, h, p, b);
            let ok = console::fb::self_test(&mut fb);
            kprintln!("ruos: fb test {}", if ok { "ok" } else { "fail" });
            console::CONSOLE.lock().attach_framebuffer(fb);
            kprintln!("ruos: fb attached");
        }
        Err(e) => {
            kprintln!("ruos: fb fail: {}", e);
        }
    }
```

`attach_framebuffer` moves the `FramebufferConsole` into the global
`CONSOLE`. Every subsequent `kprintln!` reaches both serial AND
framebuffer.

- [ ] **Step 2: Build and run**
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -15'
```
Expected serial: same as Task 2 PLUS a new `ruos: fb attached` line; `TEST_PASS`.

For visual verification, run `make run` (display GUI) — after the
`fb attached` line, the `ruos: ticks=N` text should appear on screen in
the Noto font. Earlier lines (`hello serial` etc.) won't show because the
framebuffer wasn't attached yet; that's expected for Step 8.

- [ ] **Step 3: Changelog**
Create `CHANGELOG/49-26-05-28-fb-attach.md`:
```markdown
# 49 — Attach FramebufferConsole a MultiConsole (Step 8 Task 3)

**Data:** 2026-05-28

## Cosa
- `kmain`: dopo il self-test (`fb test ok`), trasferisce ownership della
  `FramebufferConsole` dentro `CONSOLE` via `attach_framebuffer(fb)`.
  Logga `ruos: fb attached`.
- Da quel punto in poi ogni `kprintln!` raggiunge sia seriale sia
  framebuffer. Verifica visiva (`make run`): `ruos: ticks=N` appare sullo
  schermo in Noto Sans Mono Size 16.

## Perché
Terzo pezzo dello Step 8: framebuffer diventa output canale di prima
classe accanto alla seriale.

## File toccati
- kernel/src/main.rs
- CHANGELOG/49-26-05-28-fb-attach.md
```

- [ ] **Step 4: Commit**
```bash
git add kernel/src/main.rs CHANGELOG/49-26-05-28-fb-attach.md
git commit -m "feat(rust): attach FramebufferConsole to MultiConsole"
```

---

## Task 4: ANSI parser (vte) + cursor blink (timer IRQ hook)

**Files:**
- Modify: `kernel/Cargo.toml`
- Modify: `kernel/src/console/ansi.rs`
- Modify: `kernel/src/console/fb.rs`
- Modify: `kernel/src/timer.rs`
- Modify: `kernel/src/main.rs`
- Create: `CHANGELOG/50-26-05-28-fb-ansi-blink.md`

- [ ] **Step 1: Add `vte` dep**

In `kernel/Cargo.toml`:
```toml
vte = { version = "0.13", default-features = false, features = ["no_std"] }
```
(If the `no_std` feature is named differently in the resolved version — some `vte` releases use `"alloc"` or `"ansi"` — pick the one that compiles. The crate is no_std + alloc compatible.)

- [ ] **Step 2: Extend `kernel/src/console/ansi.rs` with the palette + SGR parser**

Append to `kernel/src/console/ansi.rs` (keep the existing `Rgb`, `WHITE`, `BLACK`):
```rust
/// VGA-style 16-color palette indexed 0..15: 8 dim + 8 bright.
/// Order matches CSI SGR 30-37 / 40-47 (fg/bg base) and 90-97 / 100-107
/// (bright variants).
pub const VGA_16: [Rgb; 16] = [
    Rgb { r: 0x00, g: 0x00, b: 0x00 }, // 0 black
    Rgb { r: 0xAA, g: 0x00, b: 0x00 }, // 1 red
    Rgb { r: 0x00, g: 0xAA, b: 0x00 }, // 2 green
    Rgb { r: 0xAA, g: 0x55, b: 0x00 }, // 3 yellow (brown)
    Rgb { r: 0x00, g: 0x00, b: 0xAA }, // 4 blue
    Rgb { r: 0xAA, g: 0x00, b: 0xAA }, // 5 magenta
    Rgb { r: 0x00, g: 0xAA, b: 0xAA }, // 6 cyan
    Rgb { r: 0xAA, g: 0xAA, b: 0xAA }, // 7 white (light gray)
    Rgb { r: 0x55, g: 0x55, b: 0x55 }, // 8 bright black (dark gray)
    Rgb { r: 0xFF, g: 0x55, b: 0x55 }, // 9 bright red
    Rgb { r: 0x55, g: 0xFF, b: 0x55 }, // 10 bright green
    Rgb { r: 0xFF, g: 0xFF, b: 0x55 }, // 11 bright yellow
    Rgb { r: 0x55, g: 0x55, b: 0xFF }, // 12 bright blue
    Rgb { r: 0xFF, g: 0x55, b: 0xFF }, // 13 bright magenta
    Rgb { r: 0x55, g: 0xFF, b: 0xFF }, // 14 bright cyan
    Rgb { r: 0xFF, g: 0xFF, b: 0xFF }, // 15 bright white
];

/// xterm 256-color → Rgb. 0-15 = VGA_16. 16-231 = 6x6x6 RGB cube. 232-255 =
/// 24-step grayscale.
pub fn xterm_256(idx: u8) -> Rgb {
    if idx < 16 { return VGA_16[idx as usize]; }
    if idx < 232 {
        let i = idx - 16;
        let r = (i / 36) % 6;
        let g = (i / 6)  % 6;
        let b =  i       % 6;
        let to8 = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        return Rgb { r: to8(r), g: to8(g), b: to8(b) };
    }
    let v = 8 + (idx - 232) * 10;
    Rgb { r: v, g: v, b: v }
}

/// Apply a CSI SGR parameter sequence to the current fg/bg. Unknown params
/// are ignored. Returns updated (fg, bg).
///
/// Supports:
///   0      reset (fg=WHITE, bg=BLACK)
///   30..37 set fg from VGA_16[0..7]
///   40..47 set bg from VGA_16[0..7]
///   90..97 set fg from VGA_16[8..15]
///   100..107 set bg from VGA_16[8..15]
///   38;5;N set fg from xterm_256(N)
///   48;5;N set bg from xterm_256(N)
pub fn apply_sgr(mut params: impl Iterator<Item = u16>, mut fg: Rgb, mut bg: Rgb) -> (Rgb, Rgb) {
    while let Some(p) = params.next() {
        match p {
            0 => { fg = WHITE; bg = BLACK; }
            30..=37  => fg = VGA_16[(p - 30) as usize],
            40..=47  => bg = VGA_16[(p - 40) as usize],
            90..=97  => fg = VGA_16[((p - 90) + 8) as usize],
            100..=107 => bg = VGA_16[((p - 100) + 8) as usize],
            38 => {
                if params.next() == Some(5) {
                    if let Some(idx) = params.next() {
                        fg = xterm_256(idx as u8);
                    }
                }
            }
            48 => {
                if params.next() == Some(5) {
                    if let Some(idx) = params.next() {
                        bg = xterm_256(idx as u8);
                    }
                }
            }
            _ => { /* ignore */ }
        }
    }
    (fg, bg)
}
```

- [ ] **Step 3: Extend `kernel/src/console/fb.rs` with vte::Parser + blink atomics + tick_cursor**

Add to the top of `kernel/src/console/fb.rs` (alongside existing `use` lines):
```rust
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use crate::console::ansi::{apply_sgr, WHITE, BLACK};
```

Add module-level atomics after the existing structs:
```rust
pub(crate) static FB_VIRT:        AtomicPtr<u8>  = AtomicPtr::new(core::ptr::null_mut());
pub(crate) static FB_PITCH:       AtomicU32      = AtomicU32::new(0);
pub(crate) static FB_BPP:         AtomicU32      = AtomicU32::new(0);
pub(crate) static FB_PIXEL_BGR:   AtomicBool     = AtomicBool::new(true); // true=BGR
pub(crate) static CURSOR_POS:     AtomicU64      = AtomicU64::new(0); // (col<<32)|row
pub(crate) static CURSOR_SHOWN:   AtomicBool     = AtomicBool::new(false);
pub(crate) static BLINK_COUNTER:  AtomicU64      = AtomicU64::new(0);
pub(crate) const  BLINK_DIVIDER:  u64            = 25;  // 100 Hz / 25 = 4 Hz
```

Extend `FramebufferConsole` to own a parser:
```rust
pub struct FramebufferConsole {
    info:    FbInfo,
    cols:    u32,
    rows:    u32,
    cur_col: u32,
    cur_row: u32,
    fg:      Rgb,
    bg:      Rgb,
    parser:  vte::Parser,
}
```
Update `new` to publish atomics + zero-init the parser:
```rust
impl FramebufferConsole {
    pub fn new(info: FbInfo, fg: Rgb, bg: Rgb) -> Self {
        let cols = (info.width  / glyph_width()  as u32).max(1);
        let rows = (info.height / glyph_height() as u32).max(1);
        FB_VIRT.store(info.addr, Ordering::Release);
        FB_PITCH.store(info.pitch, Ordering::Release);
        FB_BPP.store(info.bpp, Ordering::Release);
        FB_PIXEL_BGR.store(matches!(info.pixel, PixelLayout::Bgr), Ordering::Release);
        let mut me = Self {
            info, cols, rows, cur_col: 0, cur_row: 0, fg, bg,
            parser: vte::Parser::new(),
        };
        me.clear();
        me.publish_cursor();
        me
    }

    fn publish_cursor(&self) {
        let packed = ((self.cur_col as u64) << 32) | (self.cur_row as u64);
        CURSOR_POS.store(packed, Ordering::Release);
    }
}
```

Replace the existing `write_str` to drive the parser instead of looping `put_char`:
```rust
impl FramebufferConsole {
    pub fn write_str(&mut self, s: &str) {
        for b in s.bytes() {
            self.parser_advance_byte(b);
        }
        self.publish_cursor();
    }

    /// Drive one byte through the vte parser. The `mem::replace` dance
    /// is the standard workaround for vte's `Parser::advance` requiring
    /// `&mut self` while the Perform target (`self`) is also `&mut`:
    /// temporarily move the parser out, run the byte, put it back.
    /// Single-threaded boot, no reentrancy.
    fn parser_advance_byte(&mut self, b: u8) {
        let mut parser = core::mem::replace(&mut self.parser, vte::Parser::new());
        parser.advance(self, b);
        self.parser = parser;
    }
}
```

Add the Perform impl:
```rust
impl vte::Perform for FramebufferConsole {
    fn print(&mut self, ch: char) {
        if self.cur_col >= self.cols { self.newline(); }
        self.draw_glyph(self.cur_col, self.cur_row, ch, self.fg, self.bg);
        self.cur_col += 1;
    }
    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => { self.cur_col = 0; }
            b'\x08' => { if self.cur_col > 0 { self.cur_col -= 1; } }
            b'\t' => {
                self.cur_col = (self.cur_col + 8) & !7;
                if self.cur_col >= self.cols { self.newline(); }
            }
            _ => {}
        }
    }
    fn csi_dispatch(&mut self, params: &vte::Params, _i: &[u8], _ignore: bool, c: char) {
        let p1 = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(1);
        match c {
            'A' => self.cur_row = self.cur_row.saturating_sub(p1.max(1) as u32),
            'B' => {
                self.cur_row = (self.cur_row + p1.max(1) as u32).min(self.rows - 1);
            }
            'C' => {
                self.cur_col = (self.cur_col + p1.max(1) as u32).min(self.cols - 1);
            }
            'D' => self.cur_col = self.cur_col.saturating_sub(p1.max(1) as u32),
            'H' => {
                let mut it = params.iter();
                let row = it.next().and_then(|p| p.first().copied()).unwrap_or(1);
                let col = it.next().and_then(|p| p.first().copied()).unwrap_or(1);
                self.cur_row = (row.saturating_sub(1) as u32).min(self.rows - 1);
                self.cur_col = (col.saturating_sub(1) as u32).min(self.cols - 1);
            }
            'J' => {
                let arg = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(0);
                if arg == 2 { self.clear(); }
            }
            'K' => {
                let gw = glyph_width() as u32;
                let oy = self.cur_row * glyph_height() as u32;
                for col in self.cur_col..self.cols {
                    let ox = col * gw;
                    for y in oy..(oy + glyph_height() as u32) {
                        for x in ox..(ox + gw) {
                            self.pixel_write(x, y, self.bg);
                        }
                    }
                }
            }
            'm' => {
                let flat = params.iter().flat_map(|p| p.iter().copied()).collect::<alloc::vec::Vec<u16>>();
                let (fg, bg) = apply_sgr(flat.into_iter(), self.fg, self.bg);
                self.fg = fg;
                self.bg = bg;
            }
            _ => { /* drop silently */ }
        }
    }
    fn esc_dispatch(&mut self, _: &[u8], _: bool, _: u8) {}
    fn hook(&mut self, _: &vte::Params, _: &[u8], _: bool, _: char) {}
    fn put(&mut self, _: u8) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, _: &[&[u8]], _: bool) {}
}
```

Add the IRQ-safe blink tick at the bottom of `fb.rs`:
```rust
/// Called from the LAPIC timer IRQ. NO locks acquired here. Toggles the
/// cursor cell directly via the published FB pointer + cursor pos atomics.
pub fn tick_cursor() {
    let n = BLINK_COUNTER.fetch_add(1, Ordering::Relaxed);
    if n % BLINK_DIVIDER != 0 { return; }
    let base = FB_VIRT.load(Ordering::Acquire);
    if base.is_null() { return; }
    let pitch = FB_PITCH.load(Ordering::Acquire) as usize;
    let bpp_bytes = (FB_BPP.load(Ordering::Acquire) as usize) / 8;
    let pos = CURSOR_POS.load(Ordering::Acquire);
    let col = (pos >> 32) as usize;
    let row = (pos & 0xFFFF_FFFF) as usize;
    let gw = glyph_width();
    let gh = glyph_height();
    let ox = col * gw;
    let oy = row * gh;
    // XOR the bottom 2 scanlines of the cell as a thin underline cursor.
    for y in (oy + gh - 2)..(oy + gh) {
        for x in ox..(ox + gw) {
            let off = y * pitch + x * bpp_bytes;
            // SAFETY: off lies within the framebuffer; bpp_bytes is 3 or 4.
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
    CURSOR_SHOWN.fetch_xor(true, Ordering::Relaxed);
}
```

- [ ] **Step 4: Hook `tick_cursor` into the timer ISR**

Edit `kernel/src/timer.rs`. The current handler reads:
```rust
pub extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    lapic::eoi();
}
```
Change to:
```rust
pub extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    crate::console::fb::tick_cursor();
    lapic::eoi();
}
```

- [ ] **Step 5: Add an ANSI smoke test in `kmain`**

In `kernel/src/main.rs`, AFTER the `ruos: fb attached` log added in Task 3, add:
```rust
    kprintln!("\x1b[31mERR\x1b[0m hello via ansi");
    kprintln!("ruos: ansi test ok");
```

- [ ] **Step 6: Build and run**
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -20'
```
Expected serial includes the new `ansi test ok` line + `TEST_PASS`. The
`\x1b[31m`/`\x1b[0m` ANSI bytes are sent to the serial too — terminal
emulators that render them will show "ERR" in red. The Makefile `grep -qF`
is unaffected (it matches `ruos: ticks=`).

For visual verification (`make run` with display): "ERR" appears red on the
framebuffer (not on serial unless the terminal interprets ANSI). The
underline cursor at the end of the last printed cell blinks at ~4 Hz.

- [ ] **Step 7: Changelog**
Create `CHANGELOG/50-26-05-28-fb-ansi-blink.md`:
```markdown
# 50 — ANSI parser (vte) + cursor blink (Step 8 Task 4)

**Data:** 2026-05-28

## Cosa
- Aggiunta dep `vte = "0.13"` (no_std).
- `console/ansi.rs`: palette `VGA_16` + `xterm_256(idx)` + `apply_sgr`
  parser per CSI SGR (reset, 30-37/40-47/90-97/100-107, 38;5;N/48;5;N).
- `console/fb.rs`: `FramebufferConsole` integra `vte::Parser`; impl
  `vte::Perform` (print, execute per `\n`/`\r`/`\b`/`\t`, csi_dispatch per
  A/B/C/D/H/J=2/K/m). Atomics modulo-level per blink: `FB_VIRT`/`FB_PITCH`/
  `FB_BPP`/`FB_PIXEL_BGR`/`CURSOR_POS`/`CURSOR_SHOWN`/`BLINK_COUNTER`.
  `tick_cursor()` IRQ-safe (no lock): XOR delle ultime 2 scanline della
  cella cursore @ 4 Hz (100 Hz / BLINK_DIVIDER=25).
- `timer::timer_handler` ora chiama `console::fb::tick_cursor()` dopo
  `TICKS.fetch_add` e prima di `lapic::eoi`.
- `kmain`: ANSI smoke test che stampa `\x1b[31mERR\x1b[0m hello via ansi`
  + `ruos: ansi test ok`.

## Perché
Chiude lo Step 8: console framebuffer completa con escape codes e cursore
lampeggiante, pronta per shell (Step 11) e GUI (Step 13).

## File toccati
- kernel/Cargo.toml, kernel/Cargo.lock
- kernel/src/console/ansi.rs, kernel/src/console/fb.rs
- kernel/src/timer.rs
- kernel/src/main.rs
- CHANGELOG/50-26-05-28-fb-ansi-blink.md
```

- [ ] **Step 8: Commit**
```bash
git add kernel/Cargo.toml kernel/Cargo.lock kernel/src/console kernel/src/timer.rs \
        kernel/src/main.rs CHANGELOG/50-26-05-28-fb-ansi-blink.md
git commit -m "feat(rust): ANSI parser (vte) + cursor blink via timer IRQ"
```

---

## Notes for the implementer

- **All build/run via WSL** with `source $HOME/.cargo/env`.
- **`tick_cursor` runs inside the timer ISR.** It must NEVER acquire any
  lock (would deadlock if the main thread holds CONSOLE) and must NEVER
  call `kprintln!` (would re-enter the very lock it's protecting against).
  The XOR-based draw uses only atomics + direct MMIO writes.
- **Limine API drift:** `noto-sans-mono-bitmap` and `vte` are external; if
  their resolved versions name methods differently (e.g. `RasterizedChar::raster`
  vs `raster_bitmap`, `Params::iter` vs direct indexing), adapt locally and
  record the change in the task's changelog.
- **PixelLayout detection** is the most likely Task 1 stumbling block. If
  the self-test fails with all pixels rendering as `bg`, the RGB byte order
  is swapped — flip `PixelLayout::Bgr` ↔ `PixelLayout::Rgb` in
  `fb_init::init` first before deeper debugging.
- **The `parser_advance_byte` swap pattern** in Task 4 Step 3 is the
  standard `&mut self` workaround for vte: `mem::replace` temporarily
  takes ownership of the parser, `advance(self, b)` borrows `self`
  freely, then we put the parser back. Single-threaded boot, no
  reentrancy.
