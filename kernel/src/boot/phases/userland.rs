//! Phase 6 — userland: networking init + async executor (never returns).

use crate::boot::BootError;

pub fn init() -> Result<core::convert::Infallible, BootError> {
    crate::rng::init();
    crate::net::init();
    crate::binfo!("user", "net init 127.0.0.1/8 (loopback)");

    // Service manager (init/systemd-lite) registry. Must come before the
    // SSH spawn so the "ssh" builtin entry exists when we mark_running.
    crate::service::init();

    // SSH server (Step 16). Non-fatal: stub returns NotImplemented until
    // Tasks 2-8 of `docs/superpowers/specs/2026-05-30-rust-step16-ssh-design.md`
    // land. Log the outcome so we can see how far the chain reached. On
    // success, flip the registry's `ssh` entry to Running with a synthetic
    // pid (0 — the kernel-side task has no fiber pid) so the userspace
    // `service` tool reflects reality.
    match crate::ssh::spawn() {
        Ok(()) => {
            crate::binfo!("ssh", "server ready");
            crate::service::mark_running("ssh", 0);
        }
        Err(e) => crate::bwarn!("ssh", "spawn: {}", e),
    }

    crate::binfo!("user", "executor starting");

    // Quiet the on-screen console: from here on INFO no longer draws on the
    // framebuffer, only WARN/ERR do. The ring buffer (`dmesg`) and the serial
    // port still receive every line. Keeps the post-boot shell on screen
    // clean instead of interleaving kernel events (ssh connects, watchdog,
    // etc.) with user I/O — to see them, run `dmesg`.
    crate::boot::log::set_console_level(crate::boot::log::LEVEL_WARN);

    // executor::run() -> ! satisfies any return type via the never-coerce rule.
    crate::executor::run();
}
