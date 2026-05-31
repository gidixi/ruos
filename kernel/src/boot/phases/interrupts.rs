//! Phase 3 — interrupt infrastructure: PIC disable + LAPIC + IOAPIC + timer +
//! keyboard wire + STI.

use crate::boot::BootError;

pub fn init() -> Result<(), BootError> {
    let acpi = super::get_acpi_info();

    crate::pic::disable();

    crate::apic::lapic::init(acpi.lapic_base, crate::idt::VEC_SPURIOUS);
    crate::binfo!("intr", "LAPIC up base=0x{:X}", acpi.lapic_base);

    // Per-CPU bring-up for the BSP: set GS base so this_cpu() works.
    // Called AFTER lapic::init so the APIC ID register is mapped and readable.
    // AP cores are enumerated below (informational) but NOT started here.
    crate::cpu::init_bsp(0); // kernel_stack_top: forward-looking, filled per-AP later
    crate::binfo!("cpu", "cpu0 apic_id={} gs_base set", crate::cpu::this_cpu().lapic_id);

    let n = acpi.cpus.len().max(1);
    crate::binfo!("cpu", "acpi: {} CPU(s) found ({} active, {} parked)", n, 1, n.saturating_sub(1));

    crate::apic::ioapic::init(acpi.ioapic_base);
    crate::binfo!("intr", "IOAPIC up base=0x{:X}", acpi.ioapic_base);

    crate::timer::init(100)
        .map_err(|_| BootError::TimerInit("timer init failed"))?;
    crate::binfo!("intr", "timer 100 Hz");

    crate::keyboard::init(&acpi.overrides);
    crate::binfo!("intr", "keyboard IRQ1 wired overrides={}", acpi.overrides.len());

    // Enable hardware interrupts.
    x86_64::instructions::interrupts::enable();
    crate::binfo!("intr", "STI — interrupts enabled");

    Ok(())
}
