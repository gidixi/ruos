//! Structured boot logger. Format: `[T+SS.MMMs] LEVEL mod  msg`.

use core::fmt::Write;
use core::sync::atomic::{AtomicU8, Ordering};
use x86_64::instructions::interrupts::without_interrupts;

/// Numeric log levels for the console threshold gate.
pub const LEVEL_INFO: u8 = 0;
pub const LEVEL_WARN: u8 = 1;
pub const LEVEL_ERR:  u8 = 2;

/// Minimum level a message must reach to be drawn on the **framebuffer**
/// (the on-screen console). Two sinks are *never* gated and always get every
/// line: the ring buffer (`dmesg`) and the serial port (the debug/log wire,
/// which the automated tests scrape). Only the framebuffer — what a local
/// user actually stares at — is gated, same spirit as Linux `printk` vs the
/// console loglevel. Starts at INFO so the whole boot sequence is on screen;
/// `set_console_level(LEVEL_WARN)` is called once userland is up so post-boot
/// INFO chatter (ssh connects, watchdog, …) only lands in `dmesg`/serial and
/// the on-screen shell stays clean.
static CONSOLE_LEVEL: AtomicU8 = AtomicU8::new(LEVEL_INFO);

/// Raise/lower the framebuffer threshold. Never affects the ring buffer or
/// the serial port.
pub fn set_console_level(level: u8) {
    CONSOLE_LEVEL.store(level, Ordering::Relaxed);
}

/// Current framebuffer threshold.
pub fn console_level() -> u8 {
    CONSOLE_LEVEL.load(Ordering::Relaxed)
}

fn level_num(level: &str) -> u8 {
    match level {
        "WARN" => LEVEL_WARN,
        "ERR " => LEVEL_ERR,
        _      => LEVEL_INFO,
    }
}

pub fn info(module: &str, args: core::fmt::Arguments) {
    emit("INFO", module, args);
}

pub fn warn(module: &str, args: core::fmt::Arguments) {
    emit("WARN", module, args);
}

pub fn error(module: &str, args: core::fmt::Arguments) {
    emit("ERR ", module, args);
}

fn emit(level: &str, module: &str, args: core::fmt::Arguments) {
    use core::fmt::Write as _;
    without_interrupts(|| {
        let ms_total = crate::boot::clock::elapsed_ms();
        let s = ms_total / 1000;
        let ms = ms_total % 1000;
        // Format once into the ring-buffer scratch, then reuse the bytes for
        // the live sinks so we don't format the line twice.
        let mut scratch = crate::klog::Scratch::new();
        let _ = writeln!(scratch, "[T+{}.{:03}s] {} {:4} {}", s, ms, level, module, args);
        let bytes = scratch.as_bytes();
        // dmesg ring buffer: always.
        crate::klog::push(bytes);
        #[cfg(feature = "netconsole")]
        crate::net::netconsole::enqueue(bytes);
        // Live console: serial always (debug/log wire); framebuffer only
        // above the current threshold.
        let line = core::str::from_utf8(bytes).unwrap_or("<log: invalid utf8>\n");
        let mut c = crate::console::CONSOLE.lock();
        if level_num(level) >= CONSOLE_LEVEL.load(Ordering::Relaxed) {
            let _ = c.write_str(line); // both serial + framebuffer
        } else {
            c.write_serial_only(line); // serial only; keep the screen clean
        }
    });
}

#[macro_export]
macro_rules! binfo {
    ($module:literal, $($arg:tt)*) => {
        $crate::boot::log::info($module, core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! bwarn {
    ($module:literal, $($arg:tt)*) => {
        $crate::boot::log::warn($module, core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! berr {
    ($module:literal, $($arg:tt)*) => {
        $crate::boot::log::error($module, core::format_args!($($arg)*))
    };
}

/// Wasm fiber dispatch trace — opt-in via `--features wasm-trace`. Default
/// is silent so the boot log stays clean.
#[cfg(feature = "wasm-trace")]
#[macro_export]
macro_rules! wtrace {
    ($($arg:tt)*) => { $crate::kprintln!($($arg)*) };
}
#[cfg(not(feature = "wasm-trace"))]
#[macro_export]
macro_rules! wtrace {
    ($($arg:tt)*) => { () };
}
