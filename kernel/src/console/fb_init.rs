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
    // limine 0.6.3: `framebuffers()` returns `&[&Framebuffer]`.
    let fb = *resp.framebuffers().first().ok_or(FbInitError::NoFramebuffer)?;

    if fb.bpp != 32 && fb.bpp != 24 {
        return Err(FbInitError::UnsupportedBpp);
    }

    // Detect pixel layout. Most QEMU/VBox configs land on BGR (blue at
    // shift 0). Override with Rgb if the mask says otherwise.
    let pixel = if fb.red_mask_shift == 0 && fb.blue_mask_shift == 16 {
        PixelLayout::Rgb
    } else {
        PixelLayout::Bgr
    };

    let info = FbInfo {
        addr:   fb.address() as *mut u8,
        width:  fb.width  as u32,
        height: fb.height as u32,
        pitch:  fb.pitch  as u32,
        bpp:    fb.bpp    as u32,
        pixel,
    };

    Ok(FramebufferConsole::new(info, WHITE, BLACK))
}
