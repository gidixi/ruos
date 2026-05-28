use core::fmt;
use uart_16550::SerialPort;

/// Minimal COM1 (0x3F8) writer over uart_16550.
pub struct Serial {
    port: SerialPort,
}

impl Serial {
    pub const fn new() -> Self {
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

/// Globally accessible spin-locked serial writer, used by `kprintln!` and by
/// interrupt handlers. Call `SERIAL.lock().init()` exactly once at boot.
pub static SERIAL: spin::Mutex<Serial> = spin::Mutex::new(Serial::new());

// SAFETY: SerialPort accesses only its I/O port, which is exclusive-by-construction.
unsafe impl Send for Serial {}
