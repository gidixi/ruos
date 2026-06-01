//! Application Processor entry point. Limine hands each AP here already in
//! 64-bit long mode on a Limine-owned stack; we load this core's GDT/TSS and
//! the shared IDT, register online, then park in `hlt`. Fase 1: no IRQs, no
//! work — the cooperative executor stays single-core on the BSP.

use limine::mp::MpInfo;

/// AP entry. `extra_argument` carries the dense cpu_id we assigned in `bringup`.
///
/// SAFETY: invoked by Limine as the AP's `MpGotoFunction`. The BSP has already
/// called `set_cpu_mapping(lapic_id, cpu_id)` so PER_CPU[cpu_id] and the
/// LAPIC->cpu table entry exist before we run.
pub unsafe extern "C" fn ap_entry(info: &MpInfo) -> ! {
    let cpu_id = info.extra_argument() as usize;
    // Load this core's GDT/TSS (slot cpu_id) and the shared IDT.
    crate::gdt::init(cpu_id);
    crate::idt::load();
    // Register online. cpu_id() now resolves correctly on this core via the
    // LAPIC ID (mapped by the BSP before bootstrap).
    crate::cpu::mark_online();
    // Park. No STI: APs receive no interrupts in Fase 1.
    loop {
        x86_64::instructions::hlt();
    }
}
