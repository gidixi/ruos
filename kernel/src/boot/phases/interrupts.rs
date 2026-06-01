//! Phase 3 — interrupt infrastructure: PIC disable + LAPIC + IOAPIC + timer +
//! keyboard wire + STI.

use crate::boot::BootError;

pub fn init() -> Result<(), BootError> {
    let acpi = super::get_acpi_info();

    crate::pic::disable();

    crate::apic::lapic::init(acpi.lapic_base, crate::idt::VEC_SPURIOUS);
    crate::binfo!("intr", "LAPIC up base=0x{:X}", acpi.lapic_base);

    // Per-CPU bring-up for the BSP: set GS base so this_cpu() resolves via
    // gs:[0]. Called AFTER lapic::init so the APIC ID register is mapped.
    // AP cores are enumerated below (informational) but NOT started here.
    // init_bsp returns false on VMMs that silently ignore the GS-base MSR
    // (VirtualBox); this_cpu() then falls back to the BSP slot, so boot
    // continues on a single CPU regardless.
    let gs_ok = crate::cpu::init_bsp(0); // kernel_stack_top filled per-AP later
    crate::binfo!(
        "cpu", "cpu0 apic_id={} gs_base={}",
        crate::cpu::this_cpu().lapic_id,
        if gs_ok { "set" } else { "unavailable (BSP-slot fallback)" }
    );

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

    // Start the enumerated APs (Limine MpRequest) and park them idle.
    crate::smp::bringup();

    Ok(())
}
