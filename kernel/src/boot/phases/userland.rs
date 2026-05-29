//! Phase 6 — userland: networking init + async executor (never returns).

use crate::boot::BootError;

pub fn init() -> Result<core::convert::Infallible, BootError> {
    crate::rng::init();
    crate::net::init();
    crate::binfo!("user", "net init 127.0.0.1/8 (loopback)");
    crate::binfo!("user", "executor starting");

    // executor::run() -> ! satisfies any return type via the never-coerce rule.
    crate::executor::run();
}
