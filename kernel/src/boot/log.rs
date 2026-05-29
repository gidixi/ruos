//! Structured boot logger. Format: `[T+SS.MMMs] LEVEL mod  msg`.

use core::fmt::Write;
use x86_64::instructions::interrupts::without_interrupts;

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
    without_interrupts(|| {
        let ms_total = crate::boot::clock::elapsed_ms();
        let s = ms_total / 1000;
        let ms = ms_total % 1000;
        let mut c = crate::console::CONSOLE.lock();
        let _ = writeln!(c, "[T+{}.{:03}s] {} {:4} {}", s, ms, level, module, args);
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
