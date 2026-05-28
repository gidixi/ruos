//! Framebuffer console: text rendering on Limine's framebuffer.
//!
//! Task 1: printable ASCII, '\n', '\r', '\b', '\t', scrolling, clear. No
//! ANSI parsing, no cursor blink. Task 4 adds vte::Parser + cursor blink.

use core::ptr::write_volatile;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use crate::console::ansi::{apply_sgr, Rgb};
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
    info:    FbInfo,
    cols:    u32,
    rows:    u32,
    cur_col: u32,
    cur_row: u32,
    fg:      Rgb,
    bg:      Rgb,
    parser:  vte::Parser,
}

unsafe impl Send for FramebufferConsole {}

pub(crate) static FB_VIRT:       AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
pub(crate) static FB_PITCH:      AtomicU32     = AtomicU32::new(0);
pub(crate) static FB_BPP:        AtomicU32     = AtomicU32::new(0);
pub(crate) static FB_PIXEL_BGR:  AtomicBool    = AtomicBool::new(true);
pub(crate) static CURSOR_POS:    AtomicU64     = AtomicU64::new(0);
pub(crate) static CURSOR_SHOWN:  AtomicBool    = AtomicBool::new(false);
pub(crate) static BLINK_COUNTER: AtomicU64     = AtomicU64::new(0);
pub(crate) const  BLINK_DIVIDER: u64           = 25;

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

    pub fn dims(&self) -> (u32, u32, u32, u32) {
        (self.info.width, self.info.height, self.info.pitch, self.info.bpp)
    }

    pub fn info(&self) -> FbInfo { self.info }

    pub fn write_str(&mut self, s: &str) {
        for b in s.bytes() {
            self.parser_advance_byte(b);
        }
        self.publish_cursor();
    }

    /// Drive one byte through the vte parser. `mem::replace` is the standard
    /// workaround for vte's `Parser::advance` requiring `&mut self` while
    /// the Perform target is also `&mut Self`: temporarily move the parser
    /// out, run the byte, put it back. Single-threaded boot, no reentrancy.
    fn parser_advance_byte(&mut self, b: u8) {
        let mut parser = core::mem::replace(&mut self.parser, vte::Parser::new());
        parser.advance(self, b);
        self.parser = parser;
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
    let fg = fb.fg;
    let bg = fb.bg;
    fb.draw_glyph(0, 0, 'X', fg, bg);

    let raster = raster_for('X');
    let bpp_bytes = (fb.info.bpp as usize) / 8;
    let pitch = fb.info.pitch as usize;

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
                let gh = glyph_height() as u32;
                let oy = self.cur_row * gh;
                for col in self.cur_col..self.cols {
                    let ox = col * gw;
                    for y in oy..(oy + gh) {
                        for x in ox..(ox + gw) {
                            self.pixel_write(x, y, self.bg);
                        }
                    }
                }
            }
            'm' => {
                let flat = params.iter()
                    .flat_map(|p| p.iter().copied())
                    .collect::<alloc::vec::Vec<u16>>();
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

// Suppress dead_code on currently-unread atomic for now.
#[allow(dead_code)]
fn _force_use() { let _ = FB_PIXEL_BGR.load(Ordering::Relaxed); let _ = CURSOR_SHOWN.load(Ordering::Relaxed); }
