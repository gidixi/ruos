//! Disable the legacy 8259 PIC by masking every IRQ on both chips.
//! After this runs, the PIC cannot deliver interrupts; the APIC is the only
//! source allowed to fire vectors into the IDT.

use x86_64::instructions::port::Port;

pub fn disable() {
    let mut master_data: Port<u8> = Port::new(0x21);
    let mut slave_data:  Port<u8> = Port::new(0xA1);
    // SAFETY: masking the legacy PIC is idempotent and never causes spurious IRQs.
    unsafe {
        master_data.write(0xFF);
        slave_data.write(0xFF);
    }
}
