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
