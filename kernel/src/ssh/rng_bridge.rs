//! Bridge `getrandom::register_custom_getrandom!` to our kernel CSPRNG.
//!
//! getrandom uses this for ed25519-dalek key generation, ephemeral DH
//! scalars in sunset's KEX, and any other randomness inside the SSH stack.

use getrandom::register_custom_getrandom;

fn ruos_getrandom(buf: &mut [u8]) -> Result<(), getrandom::Error> {
    crate::rng::fill(buf);
    Ok(())
}

register_custom_getrandom!(ruos_getrandom);

/// Minimal `log` facade → serial, so sunset's `log::warn!`/`debug!` traces
/// surface during SSH bring-up. Installed once at SSH spawn.
struct SerialLogger;

impl log::Log for SerialLogger {
    fn enabled(&self, _m: &log::Metadata) -> bool { true }
    fn log(&self, record: &log::Record) {
        crate::kprintln!("[{}] {}", record.level(), record.args());
    }
    fn flush(&self) {}
}

static LOGGER: SerialLogger = SerialLogger;

pub fn init_logger() {
    let _ = log::set_logger(&LOGGER);
    // Warn only — sunset's Trace/Debug flood the serial and slow the
    // cooperative loop. Our verify_ed25519 patch logs at warn.
    log::set_max_level(log::LevelFilter::Warn);
}
