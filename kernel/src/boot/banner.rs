//! Boot banner. Uses ASCII chars only — the framebuffer font
//! (`noto-sans-mono-bitmap`) doesn't include Unicode box-drawing
//! glyphs, so the banner needs to render identically on serial AND
//! on the framebuffer console.

use core::fmt::Write;
use x86_64::instructions::interrupts::without_interrupts;

pub fn stamp() {
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
        let _ = writeln!(c);
        let _ = writeln!(c, "  +{:-<41}+", "");
        let _ = writeln!(c, "  | {:^39} |", format_args!("ruos v{}-dev ({}, {})", version, sha, date));
        let _ = writeln!(c, "  | {:^39} |", "x86_64-unknown-none / Limine 11.4.1");
        let _ = writeln!(c, "  | {:^39} |", format_args!("{} MiB RAM / 1 CPU / WASIX-bootstrap", ram_mib));
        let _ = writeln!(c, "  +{:-<41}+", "");
        let _ = writeln!(c);
    });
}
