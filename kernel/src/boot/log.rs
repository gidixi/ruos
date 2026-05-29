//! Structured boot logger. Format: `[T+SECS.MILLISs] L  mod: msg`.

use core::fmt::Write;
use x86_64::instructions::interrupts::without_interrupts;

pub fn info(module: &str, args: core::fmt::Arguments) {
    emit('I', module, args);
}

pub fn warn(module: &str, args: core::fmt::Arguments) {
    emit('W', module, args);
}

pub fn error(module: &str, args: core::fmt::Arguments) {
    emit('E', module, args);
}

fn emit(level: char, module: &str, args: core::fmt::Arguments) {
    without_interrupts(|| {
        let ticks = crate::timer::ticks();
        let s = ticks / 100;
        let ms = (ticks % 100) * 10;
        let mut c = crate::console::CONSOLE.lock();
        let _ = writeln!(c, "[T+{:5}.{:03}s] {}  {:8} {}", s, ms, level, module, args);
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
