//! Phase 3 — interrupt infrastructure: PIC disable + LAPIC + IOAPIC + timer +
//! keyboard wire + STI.

use crate::boot::BootError;

pub fn init() -> Result<(), BootError> {
    let acpi = super::get_acpi_info();

    crate::pic::disable();

    crate::apic::lapic::init(acpi.lapic_base, crate::idt::VEC_SPURIOUS);
    crate::binfo!("irq", "LAPIC up base=0x{:X}", acpi.lapic_base);

    crate::apic::ioapic::init(acpi.ioapic_base);
    crate::binfo!("irq", "IOAPIC up base=0x{:X}", acpi.ioapic_base);

    crate::timer::init(100)
        .map_err(|_| BootError::TimerInit("timer init failed"))?;
    crate::binfo!("irq", "timer 100 Hz");

    crate::keyboard::init(&acpi.overrides);
    crate::binfo!("irq", "keyboard IRQ1 wired overrides={}", acpi.overrides.len());

    // Enable hardware interrupts.
    x86_64::instructions::interrupts::enable();
    crate::binfo!("irq", "STI — interrupts enabled");

    Ok(())
}
