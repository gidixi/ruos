//! USB stack: xHCI controller + HID keyboard. Polled (no MSI). See
//! docs/superpowers/specs/2026-06-01-usb-xhci-hid-design.md.
pub mod control;
pub mod device;
pub mod hid;
pub mod usage;
pub mod xhci;

use crate::sync::IrqMutex;
use spin::Once;

/// Global xHCI controller handle, set once during `init()`.
pub(crate) static CTRL: Once<IrqMutex<Option<xhci::Xhci>>> = Once::new();

/// Global addressed USB device, set once after Enable Slot + Address Device.
pub(crate) static DEVICE: Once<IrqMutex<Option<device::UsbDevice>>> = Once::new();

/// Global HID boot keyboard endpoint info, set once when a HID keyboard is found.
pub(crate) static KBD: Once<IrqMutex<Option<hid::HidKeyboard>>> = Once::new();

/// Global HID keyboard running state (interrupt ring + report buf), set once
/// after Configure Endpoint succeeds.
pub(crate) static HID: Once<IrqMutex<Option<hid::HidState>>> = Once::new();

/// Bring up the xHCI controller and enumerate devices. Non-fatal: logs and
/// returns if there is no controller or bring-up fails.
pub fn init() {
    xhci::init();
}

/// Drain the event ring + process HID input. Called by `usb_poll_task`.
pub fn poll() {
    xhci::poll();
}
