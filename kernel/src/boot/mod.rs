//! Boot phase orchestration.

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
    phases::devices::init()?;
    phases::fs::init()?;
    phases::userland::init()
}
