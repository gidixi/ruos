//! Boot phase orchestration.

pub mod clock;
pub mod log;
pub mod banner;
pub mod error;
pub mod phases;

pub use error::BootError;

/// Run all boot phases in order. Returns `Ok(Infallible)` — the Ok branch is
/// unreachable because `phases::userland::init()` never returns (`executor::run()`
/// is `-> !`). Returns `Err(BootError)` if any phase fails before that.
pub fn run() -> Result<core::convert::Infallible, BootError> {
    phases::arch::init()?;
    phases::mem::init()?;
    phases::interrupts::init()?;
    phases::pci::init()?;
    phases::devices::init()?;
    phases::fs::init()?;
    // /bin off-boot: decomprime bin.bgz (modulo Limine in HHDM) in tmpfs. Non
    // dipende da storage/usb → subito dopo fs. Elimina la dipendenza dal medium.
    phases::unpack_bin::init()?;
    phases::storage::init()?;
    // USB after the framebuffer console (devices) so its bring-up logs are
    // VISIBLE on real hardware (no serial there). USB only needs PCI; it does
    // not depend on devices/fs/storage. Must still precede userland (the
    // executor that runs usb_poll_task).
    phases::usb::init()?;
    phases::userland::init()
}
