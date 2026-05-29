//! Phase 4 — devices: framebuffer console + ANSI parser attach.

use crate::boot::BootError;

pub fn init() -> Result<(), BootError> {
    match crate::console::fb_init::init() {
        Ok(mut fb) => {
            let (w, h, p, b) = fb.dims();
            crate::binfo!("dev", "fb ok {}x{} pitch={} bpp={}", w, h, p, b);

            #[cfg(feature = "boot-checks")]
            {
                let ok = crate::console::fb::self_test(&mut fb);
                crate::binfo!("dev", "fb self-test {}", if ok { "ok" } else { "fail" });
            }

            crate::console::CONSOLE.lock().attach_framebuffer(fb);
            crate::binfo!("dev", "fb attached");
        }
        Err(e) => {
            // Framebuffer is optional — serial console is always available.
            crate::bwarn!("dev", "fb unavailable: {}", e);
        }
    }

    Ok(())
}
