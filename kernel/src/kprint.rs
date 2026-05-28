//! `kprintln!` macro built on the global serial writer.
//!
//! The body runs with interrupts disabled so an interrupt handler that also
//! calls `kprintln!` (e.g. the timer or keyboard ISR added in later tasks)
//! cannot deadlock against a preempted holder of the SERIAL spin lock.

#[macro_export]
macro_rules! kprintln {
    ($($arg:tt)*) => {{
        use core::fmt::Write as _;
        ::x86_64::instructions::interrupts::without_interrupts(|| {
            let _ = writeln!($crate::serial::SERIAL.lock(), $($arg)*);
        });
    }};
}
