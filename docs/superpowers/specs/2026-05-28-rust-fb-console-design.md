# Rust Framebuffer Console + Console Trait — Design Spec

**Date:** 2026-05-28
**Milestone:** Step 8 of the Rust OS roadmap (`docs/superpowers/roadmap-rust-os.md`).
**Status:** Approved design, ready for implementation planning.

## Context

The kernel currently logs only to COM1 serial via the global
`SERIAL: spin::Mutex<Serial>` and the `kprintln!` macro. Step 8 adds a real
**framebuffer console** rendered with a modern bitmap font, plus a
`Console` trait abstraction so `kprintln!` fans out to BOTH the serial
(debug, always available) AND the framebuffer (user-facing).

The post-pivot roadmap targets WASM apps + GUI + remote SSH. Step 13 will
add a full GUI via `rlvgl`; Step 8 only delivers a text console that
supports:

- A modern monospace bitmap font (Noto Sans Mono via the
  `noto-sans-mono-bitmap` crate, height 16).
- Scrolling.
- **Blinking cursor** driven by the LAPIC timer IRQ.
- **ANSI escape codes** (CSI subset) parsed by the `vte` crate — minimum
  cursor move, clear screen/line, 16-color and 256-color foreground +
  background, reset.
- White-on-black default; color attributes from ANSI mutate fg/bg state.

The serial side keeps passing bytes through raw — the host terminal
emulator handles ANSI for SSH/diagnostic streams.

## Goals

- Limine `FramebufferRequest` consumed; framebuffer MMIO mapped via
  `memory::map_io_page`; pitch/width/height/bpp/format detected from the
  response.
- `FramebufferConsole` rendering Noto Sans Mono Size 16 glyphs to a pixel
  buffer, with scrolling and cursor-position tracking.
- `Console` trait + `SerialConsole` + `MultiConsole` static fan-out;
  `kprintln!` rerouted to fan-out.
- ANSI escape parser via `vte`; subset implemented (cursor A/B/C/D/H,
  clear `2J`/`K`, fg/bg 16-color + 256-palette, `0m` reset).
- Cursor blink at ~4 Hz, driven by the existing LAPIC timer IRQ via a
  dedicated counter (no spin-lock acquired from the ISR).
- `make run-test` keeps asserting `ruos: ticks=`. New observable serial
  lines: `ruos: fb ok WxH pitch=P bpp=B`, `ruos: fb test ok`,
  `ruos: ansi test ok`.
- Visual confirmation in VirtualBox/QEMU: boot log + colored "ERR" + a
  blinking cursor on screen.

## Non-goals (YAGNI)

- No double buffering (write directly to FB MMIO; tearing accepted).
- No mouse pointer (Step 13).
- No anti-aliased rendering; intensity from Noto raster is binarized via
  a threshold.
- No font fallback (missing glyph → render `'?'`).
- No 24-bit truecolor ANSI (`38;2;R;G;B`). Only 16-color named + 256-color
  palette indices.
- No mode-set / VESA reprogramming (use what Limine provides).
- No support for ANSI sequences beyond the implemented subset; unknown
  sequences are dropped silently by `vte`.
- No locale / Unicode beyond what the font crate provides (Noto covers
  basic Latin + many extras; non-Latin scripts may render as `'?'`).

## Architecture

```
                       kprintln!("ruos: ...")
                              |
                              v
        crate::console::CONSOLE.lock()  (spin::Mutex<MultiConsole>)
                              |
                  +-----------+----------+
                  v                      v
           SerialConsole          Option<FramebufferConsole>
                  |                      |
                  v                      v
            SERIAL (COM1)          FB MMIO (HHDM-mapped UC)
                                     + vte::Parser
                                     + cursor pos atomics
                                       (read by timer IRQ for blink)
```

### Module layout

```
kernel/src/console/
  mod.rs           # trait Console + MultiConsole + CONSOLE static + attach API
  serial_con.rs    # SerialConsole: thin wrapper over crate::serial::SERIAL
  fb.rs            # FramebufferConsole: rendering, vte::Perform, state
  fb_init.rs       # Limine FramebufferRequest static + init() builder
  font.rs          # font lookup helper (wraps noto-sans-mono-bitmap)
  ansi.rs          # color palette + Rgb type + ANSI param → color
```

The `crate::serial` module (existing) stays as-is and remains the lowest
layer; `SerialConsole` just delegates to it.

## Components

### `console::serial_con` — `SerialConsole`

```rust
pub struct SerialConsole;

impl crate::console::Console for SerialConsole {
    fn write_str(&mut self, s: &str) {
        // Pass-through to existing SERIAL global; ANSI bytes ride along
        // for the host terminal emulator to render.
        let mut s_lock = crate::serial::SERIAL.lock();
        for b in s.bytes() {
            let _ = s_lock.write_str(core::str::from_utf8(&[b]).unwrap_or("?"));
        }
    }
    fn clear(&mut self) { /* no-op for serial */ }
}
```

### `console::ansi` — palette + color types

```rust
#[derive(Debug, Copy, Clone)]
pub struct Rgb { pub r: u8, pub g: u8, pub b: u8 }

pub const WHITE: Rgb = Rgb { r: 0xEE, g: 0xEE, b: 0xEE };
pub const BLACK: Rgb = Rgb { r: 0x00, g: 0x00, b: 0x00 };

/// VGA 16-color palette indexed 0..15 (8 dim + 8 bright).
pub const VGA_16: [Rgb; 16] = [ /* black, red, green, ..., bright_white */ ];

/// xterm 256-color palette index → Rgb.
pub fn xterm_256(idx: u8) -> Rgb;

/// Parse a CSI SGR parameter sequence into fg/bg mutations.
/// Returns (new_fg, new_bg) given old values + the params slice from vte.
pub fn apply_sgr(params: &[u16], cur_fg: Rgb, cur_bg: Rgb) -> (Rgb, Rgb);
```

### `console::font` — font lookup

```rust
use noto_sans_mono_bitmap::{
    get_raster, FontWeight, RasterHeight, RasterizedChar,
};

pub const FONT_HEIGHT: RasterHeight = RasterHeight::Size16;
pub const FONT_WEIGHT: FontWeight   = FontWeight::Regular;
pub const GLYPH_WIDTH: usize = noto_sans_mono_bitmap::get_raster_width(
    FontWeight::Regular, RasterHeight::Size16);
pub const GLYPH_HEIGHT: usize = FONT_HEIGHT.val();

/// Pre-resolved raster for the missing-glyph fallback.
const FALLBACK_CH: char = '?';

pub fn raster_for(ch: char) -> RasterizedChar {
    get_raster(ch, FONT_WEIGHT, FONT_HEIGHT)
        .unwrap_or_else(|| get_raster(FALLBACK_CH, FONT_WEIGHT, FONT_HEIGHT).unwrap())
}
```

(`get_raster_width` may be a runtime fn in 0.x; if not const, compute on
init and store in `static GLYPH_WIDTH: AtomicUsize`.)

### `console::fb` — `FramebufferConsole`

State:
```rust
pub struct FramebufferConsole {
    fb_virt:  *mut u8,    // HHDM-mapped UC virt addr from map_io_page
    width:    u32,        // pixels
    height:   u32,
    pitch:    u32,        // bytes per scanline
    bpp:      u32,        // 24 or 32
    pixel:    PixelLayout, // RGB | BGR
    cols:     u32,        // width / GLYPH_WIDTH
    rows:     u32,        // height / GLYPH_HEIGHT
    cur_col:  u32,
    cur_row:  u32,
    fg:       Rgb,        // current foreground (default WHITE)
    bg:       Rgb,        // current background (default BLACK)
    parser:   vte::Parser,
}
```

The pointer + dimensions are also stored in module-level **atomics** for
the timer-IRQ blink to read without locking:
```rust
pub(crate) static FB_VIRT:     AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
pub(crate) static FB_PITCH:    AtomicU32    = AtomicU32::new(0);
pub(crate) static FB_BPP:      AtomicU32    = AtomicU32::new(0);
pub(crate) static FB_PIXEL:    AtomicU32    = AtomicU32::new(0); // 0=RGB, 1=BGR
pub(crate) static CURSOR_POS:  AtomicU64    = AtomicU64::new(0); // (col<<32) | row
pub(crate) static CURSOR_SHOWN: AtomicBool  = AtomicBool::new(false);
pub(crate) static BLINK_COUNTER: AtomicU64  = AtomicU64::new(0);
pub(crate) const BLINK_DIVIDER: u64 = 25;  // 100 Hz / 25 = 4 Hz blink
```

Public API:
```rust
impl FramebufferConsole {
    pub fn new(info: FbInfo) -> Self;          // clears to black, cursor=(0,0)
    pub fn write_str(&mut self, s: &str);      // feeds bytes through vte::Parser
    pub fn clear(&mut self);                   // fill black, cursor=(0,0)
}
```

Internal:
- `draw_glyph(col, row, ch, fg, bg)` — render a glyph at the cell.
- `scroll_up(rows)` — memcpy rows × pitch upward, clear bottom band.
- `put_char(ch)` — handles `\n`/`\r`/`\b`/`\t` directly; printable goes
  through `draw_glyph` with current fg/bg.
- `pixel_write(x, y, rgb)` — pack `Rgb` according to `PixelLayout` and
  bpp, write to `fb_virt + y * pitch + x * (bpp/8)`.

ANSI integration (`vte::Perform`):
```rust
impl vte::Perform for FramebufferConsole {
    fn print(&mut self, ch: char) { self.put_printable(ch); }
    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.carriage_return(),
            b'\x08' => self.backspace(),
            b'\t'   => self.tab(),
            _ => {}
        }
    }
    fn csi_dispatch(&mut self, params: &vte::Params, _i: &[u8], _ignore: bool, c: char) {
        match c {
            'A' => self.cursor_up(first_param(params, 1)),
            'B' => self.cursor_down(first_param(params, 1)),
            'C' => self.cursor_forward(first_param(params, 1)),
            'D' => self.cursor_back(first_param(params, 1)),
            'H' => self.cursor_to(rc_params(params)),
            'J' if first_param(params, 0) == 2 => self.clear(),
            'K' => self.clear_line_to_eol(),
            'm' => self.apply_sgr(params),
            _   => { /* drop silently */ }
        }
    }
    // esc_dispatch, hook, osc_dispatch, unhook, put: no-op stubs
}
```

### `console::fb_init` — Limine request + init

```rust
pub static FRAMEBUFFER_REQUEST: limine::request::FramebufferRequest =
    limine::request::FramebufferRequest::new();

pub fn init() -> Result<FramebufferConsole, FbInitError>;

pub enum FbInitError { NoResponse, NoFramebuffer, UnsupportedBpp, MapFailed }
```

The request static lives in main.rs alongside the other Limine requests,
inside the existing marker bracket (consistent with HHDM/MEMMAP/RSDP).

`init()` reads the first framebuffer entry, calls
`memory::map_io_page(phys)` if Limine's address is not yet HHDM-mapped (in
practice Limine maps the framebuffer through HHDM, so the existing
`hhdm_offset + address` works — we verify by trying a probe write/read).

### `console::mod` — Console trait + MultiConsole + global

```rust
pub trait Console {
    fn write_str(&mut self, s: &str);
    fn clear(&mut self);
}

pub struct MultiConsole {
    pub serial: SerialConsole,
    pub fb:     Option<FramebufferConsole>,
}

impl MultiConsole {
    pub const fn new() -> Self { Self { serial: SerialConsole, fb: None } }
    pub fn attach_framebuffer(&mut self, fb: FramebufferConsole) {
        self.fb = Some(fb);
    }
}

impl core::fmt::Write for MultiConsole {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.serial.write_str(s);
        if let Some(fb) = &mut self.fb { fb.write_str(s); }
        Ok(())
    }
}

pub static CONSOLE: spin::Mutex<MultiConsole> = spin::Mutex::new(MultiConsole::new());
```

### `kprintln!` refactor

`kernel/src/kprint.rs`: change `$crate::serial::SERIAL.lock()` to
`$crate::console::CONSOLE.lock()`. Body still wrapped in
`without_interrupts(|| ...)`.

### Cursor blink hook (timer IRQ)

`kernel/src/timer.rs`'s `timer_handler` is extended:
```rust
pub extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    crate::console::fb::tick_cursor();  // new
    lapic::eoi();
}
```
`fb::tick_cursor()`:
```rust
pub fn tick_cursor() {
    let n = BLINK_COUNTER.fetch_add(1, Ordering::Relaxed);
    if n % BLINK_DIVIDER != 0 { return; }
    let fb = FB_VIRT.load(Ordering::Relaxed);
    if fb.is_null() { return; }  // FB not yet attached
    let shown = CURSOR_SHOWN.fetch_xor(true, Ordering::Relaxed);
    let pos = CURSOR_POS.load(Ordering::Relaxed);
    let col = (pos >> 32) as u32;
    let row = pos as u32;
    // XOR cursor cell rectangle: invert pixels in the GLYPH_WIDTH×GLYPH_HEIGHT
    // cell, no lock acquired. Direct MMIO writes via fb pointer + pitch.
}
```

No lock held → cannot deadlock with `kprintln!` writing through CONSOLE.
Race-vs-`put_char`-on-same-cell window is rare and self-corrects on next
blink tick (acceptable Step 8).

## Cargo.toml deps

Add:
```toml
noto-sans-mono-bitmap = "0.3"  # or latest compatible
vte = "0.13"                    # or latest no_std-compatible
```

(Exact versions resolved in the plan.)

## Boot sequence in kmain

Insert after `vfs::block_on(smoke)` and the resulting smoke log,
BEFORE the busy-wait on `timer::ticks()`:

```rust
match console::init_framebuffer() {
    Ok(info) => kprintln!(
        "ruos: fb ok {}x{} pitch={} bpp={}",
        info.width, info.height, info.pitch, info.bpp,
    ),
    Err(e) => kprintln!("ruos: fb fail: {}", e),
}
// Self-test (only if FB attached): render 'X', verify, log.
if let Some(_) = console::CONSOLE.lock().fb.as_mut() {
    let ok = console::fb_self_test();
    kprintln!(if ok { "ruos: fb test ok" } else { "ruos: fb test fail" });
}
// ANSI parser smoke test:
kprintln!("\x1b[31mERR\x1b[0m hello via ansi");
kprintln!("ruos: ansi test ok");
```

`console::init_framebuffer` internally calls `fb_init::init()` and, on
success, attaches the returned `FramebufferConsole` into `CONSOLE`.

## Errors

- `FbInitError` variants (`NoResponse`, `NoFramebuffer`, `UnsupportedBpp`,
  `MapFailed`) all log via `kprintln!` and continue — serial-only fallback
  is fine.
- ANSI parser never panics; unknown sequences drop silently.
- Glyph rasterizer returns `None` for unsupported chars → fallback to
  `'?'` (handled in `font::raster_for`).

## Testing

- **Headless (`make run-test`)**: assertion `ruos: ticks=` unchanged.
  Reaching it implies `fb ok`, `fb test ok` (or `fb fail` — non-fatal),
  and the ANSI test line all printed via the new console.
- **`fb test ok` self-test**: render `'X'` at (0,0); read back the pixel
  bytes for the glyph cell from FB MMIO; compare to the Noto raster
  bytes after threshold. Either matches → `ok`, or fails → diagnostic.
- **Visual** (VBox/QEMU display, `make run`): the whole boot log shows on
  the framebuffer in the modern font, with "ERR" rendered red. Cursor at
  end of last line blinks at ~4 Hz.

## Decomposition into tasks

1. **FB low-level** — `fb_init.rs` (Limine request + init), `font.rs`,
   `fb.rs` minimal (`new`/`put_char` for printable ASCII + `\n`/`\r`,
   `scroll_up`, `clear`, `draw_glyph`, `pixel_write`). NO Console trait
   yet, NO vte yet, NO blink. Boot: log `fb ok` + `fb test ok`. kmain
   uses the new FB directly only for the self-test, not for logging.
2. **Console trait + MultiConsole + SerialConsole + `kprintln!` refactor**.
   FB still NOT attached. TEST_PASS unchanged because behavior is
   serial-only equivalent.
3. **Attach FB to MultiConsole** — `console::init_framebuffer` constructs
   `FramebufferConsole` via Task 1's API and stores it inside `CONSOLE`.
   From here on, every `kprintln!` after attach also draws to FB. Visual
   confirmation only.
4. **ANSI parser via `vte` + cursor blink** — add `vte::Parser` field to
   `FramebufferConsole`, route bytes through `vte::Perform`. Implement
   the listed CSI subset. Hook `timer::timer_handler` to call
   `fb::tick_cursor`. Smoke test prints `\x1b[31mERR\x1b[0m hello via ansi`
   + `ruos: ansi test ok`.

## Open items for the implementation plan

- Resolved versions of `noto-sans-mono-bitmap` and `vte` against the
  pinned nightly.
- Whether `get_raster_width` is `const fn` in the resolved crate version
  (fallback: read once at init, store in atomic).
- Exact `PixelLayout` derivation from Limine's `memory_model` field
  (typically `Bgr8`/`Rgb8` indicators) — the plan should pin the field
  names against `limine` 0.6.3.
- Whether the cursor blink rectangle XOR uses 2 bytes/pixel or 4
  (depending on `bpp`); the plan must handle both 24-bit and 32-bit.
