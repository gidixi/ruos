# 180 — Non-deadlocking, auto-resetting panic handler

**Data:** 2026-05-31

## Cosa
Rewrote the kernel `#[panic_handler]` in `kernel/src/main.rs`:

- Calls `x86_64::instructions::interrupts::disable()` first (was already
  present; kept).
- Formats the panic message once into a fixed 256-byte stack buffer
  (`klog::Scratch`) to avoid any heap allocation.
- Appends to the klog ring via the new `klog::try_push()` — best-effort,
  no-op if the ring lock is contested.
- Writes to `SERIAL` via `try_lock()` — skips if contested (never blocks).
- Writes to the framebuffer console directly via
  `FramebufferConsole::write_str()` after a `CONSOLE.try_lock()` — bypasses
  the `SerialConsole` inner path that calls `SERIAL.lock()` unconditionally,
  which would deadlock if serial is held.
- **Bug fixed**: old code used `CONSOLE.lock()` unconditionally, which would
  deadlock whenever the panic was triggered inside any kprintln/logging call
  (a very common case).
- Default behaviour: calls `crate::power::reboot()` after printing — the box
  auto-recovers instead of hanging forever until a power-cycle.
- New `panic-halt` feature: gates the old `hlt` loop for inspection under a
  debugger. `--features panic-halt` restores halt-on-panic.

Added `klog::try_push(bytes: &[u8])` — non-blocking push that uses
`try_lock` internally; safe to call from panic context.

Added `panic-halt = []` to `kernel/Cargo.toml` `[features]`.

## Perché
A kernel panic held the CONSOLE spin-lock unconditionally, meaning any panic
that fired while a logging call was in progress (the common case) would
silently deadlock — no panic message ever appeared and the machine hung
permanently. Changing all lock acquisitions to `try_lock` eliminates every
blocking path in the panic handler. The auto-reboot default means a panicking
kernel degrades to a reboot rather than a permanent brick.

## File toccati
- kernel/src/main.rs
- kernel/src/klog.rs
- kernel/Cargo.toml
