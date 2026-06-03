//! Framebuffer console: text rendering on Limine's framebuffer.
//!
//! Task 8: FramebufferConsole wired onto Grid + Surface + GlyphCache + render
//! pipeline built in Tasks 2-7. tick_cursor stays untouched (reads atomics).

use core::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, Ordering};
use crate::console::ansi::{apply_sgr, Rgb};
use crate::console::font::{glyph_height, glyph_width};
use crate::console::grid::Grid;
use crate::console::surface::Surface;
use crate::console::glyphcache::GlyphCache;
use crate::console::render;

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
    info:   FbInfo,
    grid:   Grid,
    surf:   Surface,
    cache:  GlyphCache,
    parser: vte::Parser,
}

unsafe impl Send for FramebufferConsole {}

pub(crate) static FB_VIRT:       AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
pub(crate) static FB_PITCH:      AtomicU32     = AtomicU32::new(0);
pub(crate) static FB_BPP:        AtomicU32     = AtomicU32::new(0);
pub(crate) static CURSOR_POS:    AtomicU64     = AtomicU64::new(0);
pub(crate) static BLINK_COUNTER: AtomicU64     = AtomicU64::new(0);
// 100 Hz LAPIC timer / 50 = ~2 Hz blink (one blink per ~500 ms — calm,
// terminal-like). Was /25 = 4 Hz; before the SMP AP-idle fix the busy-spinning
// APs starved the BSP, so the timer ran below nominal and the cursor looked
// slow. With APs now hlt-idle the BSP gets the full 100 Hz, exposing the
// designed-but-fast 4 Hz; 2 Hz reads better.
pub(crate) const  BLINK_DIVIDER: u64           = 50;

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
        me.clear();
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

    #[cfg(feature = "boot-checks")]
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
}
