//! `kprintln!` macro built on the global multi-console writer.
//!
//! The body runs with interrupts disabled so an interrupt handler that also
//! calls `kprintln!` (e.g. the timer or keyboard ISR) cannot deadlock
//! against a preempted holder of the CONSOLE spin lock.

#[macro_export]
macro_rules! kprintln {
    ($($arg:tt)*) => {{
        use core::fmt::Write as _;
        ::x86_64::instructions::interrupts::without_interrupts(|| {
            let _ = writeln!($crate::console::CONSOLE.lock(), $($arg)*);
        });
    }};
}
