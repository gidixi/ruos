//! Phase — USB: bring up the xHCI controller + enumerate HID devices. Runs
//! after `pci` (needs the xHCI BAR). Non-fatal: a machine without xHCI boots
//! fine (USB is additive to the PS/2 keyboard).
use crate::boot::BootError;

pub fn init() -> Result<(), BootError> {
    crate::usb::init();
    Ok(())
}
