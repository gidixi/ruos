//! Console subsystem: framebuffer rendering, ANSI parsing, fan-out logging.

pub mod ansi;
pub mod boxdraw;
pub mod font;
pub mod glyphcache;
pub mod grid;
pub mod fb;
pub mod fb_init;
pub mod serial_con;
pub mod surface;
pub mod render;
pub mod engine_test;

use core::fmt;
use spin::Mutex;
use crate::console::fb::FramebufferConsole;
use crate::console::serial_con::SerialConsole;

pub trait Console {
    fn write_str(&mut self, s: &str);
    fn clear(&mut self);
}

pub struct MultiConsole {
    pub serial: SerialConsole,
    pub fb:     Option<FramebufferConsole>,
}

impl MultiConsole {
    pub const fn new() -> Self {
        Self { serial: SerialConsole, fb: None }
    }

    /// Stash a constructed FramebufferConsole. From now on every write_str
    /// also reaches the framebuffer. Called by Task 3 wiring in kmain.
    pub fn attach_framebuffer(&mut self, fb: FramebufferConsole) {
        self.fb = Some(fb);
    }

    /// Write to the serial port only, skipping the framebuffer. Used by the
    /// log emitter to keep the serial a full debug/log wire (every line)
    /// while the on-screen framebuffer console stays quiet for post-boot
    /// INFO chatter. See `boot::log::emit`.
    pub fn write_serial_only(&mut self, s: &str) {
        Console::write_str(&mut self.serial, s);
    }

    /// Write to the framebuffer only, skipping the serial port. Used to
    /// re-paint the boot banner on screen after the framebuffer attaches
    /// without duplicating it on the serial wire (serial got it pre-fb).
    pub fn write_fb_only(&mut self, s: &str) {
        if let Some(fb) = &mut self.fb {
            Console::write_str(fb, s);
        }
    }
}

impl fmt::Write for MultiConsole {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        Console::write_str(&mut self.serial, s);
        if let Some(fb) = &mut self.fb {
            Console::write_str(fb, s);
        }
        Ok(())
    }
}

// FramebufferConsole exposes inherent `write_str`/`clear`; expose them via
// the Console trait too so MultiConsole can dispatch uniformly.
impl Console for FramebufferConsole {
    fn write_str(&mut self, s: &str) { FramebufferConsole::write_str(self, s); }
    fn clear(&mut self)              { FramebufferConsole::clear(self); }
}

pub static CONSOLE: Mutex<MultiConsole> = Mutex::new(MultiConsole::new());
