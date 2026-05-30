//! Phase 6 — userland: networking init + async executor (never returns).

use crate::boot::BootError;

pub fn init() -> Result<core::convert::Infallible, BootError> {
    crate::rng::init();
    crate::net::init();
    crate::binfo!("user", "net init 127.0.0.1/8 (loopback)");

    // SSH server (Step 16). Non-fatal: stub returns NotImplemented until
    // Tasks 2-8 of `docs/superpowers/specs/2026-05-30-rust-step16-ssh-design.md`
    // land. Log the outcome so we can see how far the chain reached.
    match crate::ssh::spawn() {
        Ok(()) => crate::binfo!("ssh", "server ready"),
        Err(e) => crate::bwarn!("ssh", "spawn: {}", e),
    }

    crate::binfo!("user", "executor starting");

    // executor::run() -> ! satisfies any return type via the never-coerce rule.
    crate::executor::run();
}
