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

/// Fixed-capacity stack writer: lets us format a line, THEN center it as a
/// `&str`. Centering with `{:^39}` directly on a `format_args!` does NOT work —
/// `Display for Arguments` ignores the formatter's width/fill flags, so the
/// dynamic lines came out unpadded (and the box borders didn't line up).
struct LineBuf { bytes: [u8; 96], len: usize }
impl LineBuf {
    fn new() -> Self { Self { bytes: [0; 96], len: 0 } }
    fn as_str(&self) -> &str { core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("") }
}
impl Write for LineBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.bytes.len() - self.len);
        self.bytes[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

fn render<W: Write>(w: &mut W, version: &str, sha: &str, date: &str, ram_mib: u64) {
    let mut l1 = LineBuf::new();
    let _ = write!(l1, "ruOS v{}-dev ({}, {})", version, sha, date);
    let mut l3 = LineBuf::new();
    let _ = write!(l3, "{} MiB RAM / 1 CPU / WASIX-bootstrap", ram_mib);

    let _ = writeln!(w);
    let _ = writeln!(w, "  +{:-<41}+", "");
    let _ = writeln!(w, "  | {:^39} |", l1.as_str());
    let _ = writeln!(w, "  | {:^39} |", "x86_64-unknown-none / Limine 11.4.1");
    let _ = writeln!(w, "  | {:^39} |", l3.as_str());
    let _ = writeln!(w, "  +{:-<41}+", "");
    let _ = writeln!(w);
}
