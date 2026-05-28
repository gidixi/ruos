//! Console impl that delegates to the existing SERIAL global. ANSI escape
//! bytes are sent raw so host terminal emulators (or SSH clients) render
//! them on the other end.

use core::fmt::Write as _;

pub struct SerialConsole;

impl crate::console::Console for SerialConsole {
    fn write_str(&mut self, s: &str) {
        let _ = crate::serial::SERIAL.lock().write_str(s);
    }
    fn clear(&mut self) { /* no-op on serial */ }
}
