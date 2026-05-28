//! `kprintln!` macro built on the global serial writer.

#[macro_export]
macro_rules! kprintln {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = writeln!($crate::serial::SERIAL.lock(), $($arg)*);
    }};
}
