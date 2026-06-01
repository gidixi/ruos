//! Application Processor entry point. Limine hands each AP here already in
//! 64-bit long mode on a Limine-owned stack; we load this core's GDT/TSS and
//! the shared IDT, enable this core's LAPIC, register online, then enter a
//! compute WORKER loop (Fase 2). APs pull pure-CPU jobs from the shared pool
//! and run them on their core; when the queue is empty they `hlt` (0% CPU) and
//! sleep until the BSP wakes them with an IPI after submitting a job. The AP's
//! only enabled interrupt is the wake IPI (its timer LVT is masked, keyboard
//! IRQ routes to the BSP).

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
    // Enable this core's LAPIC (so the wake IPI is serviced) + mask its timer.
    crate::apic::lapic::init_ap(crate::idt::VEC_SPURIOUS);
    // Register online. cpu_id() now resolves correctly on this core via the
    // LAPIC ID (mapped by the BSP before bootstrap).
    crate::cpu::mark_online();
    ap_worker_loop()
}

/// AP worker loop: drain pure-CPU jobs from the pool, then `hlt` until a wake
/// IPI arrives. The BSP sends the wake IPI on every `submit`. Anti-missed-wake:
/// disable IRQs and re-check the queue before sleeping, so a job submitted
/// between the drain and the `hlt` is not missed (the `sti; hlt` is atomic —
/// the IPI cannot fire in the 1-instruction shadow of `sti`).
fn ap_worker_loop() -> ! {
    let me = crate::cpu::cpu_id() as usize;
    loop {
        // Drain all available jobs, charging their run time as busy.
        while let Some(slot) = crate::smp::pool::take() {
            let busy_start = crate::boot::clock::read_tsc();
            crate::smp::pool::run_slot(slot, me as u32);
            crate::sched::cpustat::add_busy(
                me, crate::boot::clock::read_tsc().saturating_sub(busy_start));
        }
        // No work: sleep until woken, charging the halt as idle.
        x86_64::instructions::interrupts::disable();
        if crate::smp::pool::is_empty() {
            let idle_start = crate::boot::clock::read_tsc();
            // Atomic sti;hlt — the wake IPI cannot land between the two.
            x86_64::instructions::interrupts::enable_and_hlt();
            crate::sched::cpustat::add_idle(
                me, crate::boot::clock::read_tsc().saturating_sub(idle_start));
        } else {
            // A job arrived during the drain/disable window; re-enable and loop.
            x86_64::instructions::interrupts::enable();
        }
    }
}
