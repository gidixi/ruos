//! Minimal PS/2 keyboard: read raw scancodes from port 0x60 and log them.
//! IRQ1 is wired to `VEC_KEYBOARD` via IOAPIC redirection.

use x86_64::instructions::port::Port;
use x86_64::structures::idt::InterruptStackFrame;
use crate::{apic, idt, kprintln};
use crate::acpi_init::IrqOverride;

pub extern "x86-interrupt" fn keyboard_handler(_frame: InterruptStackFrame) {
    let mut data: Port<u8> = Port::new(0x60);
    // SAFETY: 0x60 is the PS/2 controller data port.
    let scancode = unsafe { data.read() };
    kprintln!("ruos: kb scancode=0x{:X}", scancode);
    apic::lapic::eoi();
}

pub fn init(overrides: &[IrqOverride]) {
    apic::ioapic::redirect(1, idt::VEC_KEYBOARD, overrides);
}
