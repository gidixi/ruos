//! Phase 4 — devices: framebuffer console + ANSI parser attach.

use crate::boot::BootError;

pub fn init() -> Result<(), BootError> {
    match crate::console::fb_init::init() {
        Ok(mut fb) => {
            let (w, h, p, b) = fb.dims();
            crate::binfo!("dev", "fb ok {}x{} pitch={} bpp={}", w, h, p, b);
            // Capture the REAL framebuffer geometry for the GUI service before
            // any boot-checks engine_test clobbers console::fb's statics.
            crate::gfx::init(fb.info());

            #[cfg(feature = "boot-checks")]
            {
                let ok = crate::console::fb::self_test(&mut fb);
                crate::binfo!("dev", "fb self-test {}", if ok { "ok" } else { "fail" });
            }

            crate::console::CONSOLE.lock().attach_framebuffer(fb);
            crate::binfo!("dev", "fb attached");
            // Re-paint the banner on the framebuffer only (the initial stamp
            // went to serial since fb was off then). fb-only avoids a second
            // copy on the serial wire.
            crate::boot::banner::stamp_fb_only();
        }
        Err(e) => {
            // Framebuffer is optional — serial console is always available.
            crate::bwarn!("dev", "fb unavailable: {}", e);
        }
    }

    #[cfg(feature = "boot-checks")]
    crate::console::engine_test::run();

    // Self-test del kernel event bus (ring + cursori + rilevamento gap). Gira
    // PRIMA che il compositor parta: il suo cursore parte da current_seq(),
    // quindi questi eventi di prova non diventano mai toast.
    #[cfg(feature = "boot-checks")]
    crate::kevent::self_test();

    Ok(())
}
