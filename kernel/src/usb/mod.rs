//! USB stack: xHCI controller + HID keyboard. Polled (no MSI). See
//! docs/superpowers/specs/2026-06-01-usb-xhci-hid-design.md.
pub mod control;
pub mod device;
pub mod encoding;
pub mod hid;
pub mod hub;
pub mod registry;
pub mod usage;
pub mod xhci;

use crate::sync::IrqMutex;
use spin::Once;

/// Global xHCI controller handle, set once during `init()`. Per-device state now
/// lives in `registry` (keyed by slot), not in singletons — hot-plug needs N
/// devices, and the worklist drives enumeration through `registry::insert`.
pub(crate) static CTRL: Once<IrqMutex<Option<xhci::Xhci>>> = Once::new();

/// Bring up the xHCI controller and enumerate devices. Non-fatal: logs and
/// returns if there is no controller or bring-up fails.
pub fn init() {
    xhci::init();
}

/// Drain the event ring + process HID input. Called by `usb_poll_task`.
pub fn poll() {
    xhci::poll();
}
