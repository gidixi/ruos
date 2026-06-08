//! Boot banner. Uses ASCII chars only — the framebuffer font
//! (`noto-sans-mono-bitmap`) doesn't include Unicode box-drawing
//! glyphs, so the banner needs to render identically on serial AND
//! on the framebuffer console.

use core::fmt::Write;
use x86_64::instructions::interrupts::without_interrupts;

/// Stamp the banner on every console sink (serial + framebuffer). Called once
/// at boot, before the framebuffer exists, so in practice this reaches serial.
pub fn stamp() {
    emit(false);
}

/// Re-paint the banner on the framebuffer ONLY. Called once the framebuffer
/// console attaches, so it shows on screen without duplicating on the serial
/// wire (serial already got it via `stamp()` pre-fb).
pub fn stamp_fb_only() {
    emit(true);
}

/// `fmt::Write` adapter that routes only to the framebuffer. Lets `render`
/// stay generic without an `alloc` scratch buffer — `stamp()` runs before the
/// heap is up, so a `String` is not available here.
struct FbOnly<'a>(&'a mut crate::console::MultiConsole);
impl Write for FbOnly<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.write_fb_only(s);
        Ok(())
    }
}

fn emit(fb_only: bool) {
    let version = env!("CARGO_PKG_VERSION");
    let sha = option_env!("RUOS_GIT_SHA").unwrap_or("unknown");
    let date = option_env!("RUOS_BUILD_DATE").unwrap_or("unknown");

    // RAM info: query Limine memmap directly — available pre-mem-phase
    // since Limine populates the response at boot. SMP not detected; CPU=1.
    let ram_mib = crate::MEMMAP_REQUEST.response()
        .map(|r| r.entries().iter()
            .filter(|e| e.type_ == limine::memmap::MEMMAP_USABLE)
            .map(|e| e.length).sum::<u64>())
        .unwrap_or(0) / 1024 / 1024;

    without_interrupts(|| {
        let mut c = crate::console::CONSOLE.lock();
        if fb_only {
            render(&mut FbOnly(&mut c), version, sha, date, ram_mib);
        } else {
            render(&mut *c, version, sha, date, ram_mib);
        }
    });
}

fn render<W: Write>(w: &mut W, version: &str, sha: &str, date: &str, ram_mib: u64) {
    let _ = writeln!(w);
    let _ = writeln!(w, "  +{:-<41}+", "");
    let _ = writeln!(w, "  | {:^39} |", format_args!("ruos v{}-dev ({}, {})", version, sha, date));
    let _ = writeln!(w, "  | {:^39} |", "x86_64-unknown-none / Limine 11.4.1");
    let _ = writeln!(w, "  | {:^39} |", format_args!("{} MiB RAM / 1 CPU / WASIX-bootstrap", ram_mib));
    let _ = writeln!(w, "  +{:-<41}+", "");
    let _ = writeln!(w);
}
