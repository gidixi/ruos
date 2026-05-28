//! Framebuffer console: text rendering on Limine's framebuffer.
//!
//! Task 1: printable ASCII, '\n', '\r', '\b', '\t', scrolling, clear. No
//! ANSI parsing, no cursor blink. Task 4 adds vte::Parser + cursor blink.

use core::ptr::write_volatile;
use crate::console::ansi::Rgb;
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
