use core::fmt;
use uart_16550::SerialPort;

/// Minimal COM1 (0x3F8) writer over uart_16550.
pub struct Serial {
    port: SerialPort,
}

impl Serial {
    pub fn new() -> Self {
        // Safety: 0x3F8 is the standard COM1 I/O port base.
        Serial { port: unsafe { SerialPort::new(0x3F8) } }
    }

    pub fn init(&mut self) {
        self.port.init();
    }
}

impl fmt::Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            self.port.send(b);
        }
        Ok(())
    }
}
