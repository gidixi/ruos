//! Application Processor entry point. Limine hands each AP here already in
//! 64-bit long mode on a Limine-owned stack; we load this core's GDT/TSS and
//! the shared IDT, enable this core's LAPIC, arm this core's LAPIC timer in
//! periodic mode (100 Hz, same calibrated count as the BSP), register online,
//! then enter a compute WORKER loop (Fase 2). APs pull pure-CPU jobs from the
//! shared pool and run them on their core; when the queue is empty they `hlt`
//! (0% CPU) and sleep until the BSP or the periodic timer wakes them. The timer
//! IPI also drains this core's `PER_CORE_DELAYS` list on each tick (Step 3a).

use limine::mp::MpInfo;

/// AP entry. `extra_argument` carries the dense cpu_id we assigned in `bringup`.
///
/// SAFETY: invoked by Limine as the AP's `MpGotoFunction`. The BSP has already
/// called `set_cpu_mapping(lapic_id, cpu_id)` so PER_CPU[cpu_id] and the
/// LAPIC->cpu table entry exist before we run.
pub unsafe extern "C" fn ap_entry(info: &MpInfo) -> ! {
    let cpu_id = info.extra_argument() as usize;
    // FIRST: prime this AP's IA32_TSC_AUX with its dense id so the fast RDTSCP
    // `cpu_id()` path (already enabled globally by the BSP via RDTSCP_OK) returns
    // the correct id on this core. Must precede ANY `cpu_id()` call here — a
    // no-op if RDTSCP is unavailable, in which case `cpu_id()` uses the LAPIC
    // fallback. Without this, an AP could read a stale TSC_AUX=0 and corrupt
    // per-core state.
    crate::cpu::set_tsc_aux(cpu_id as u32);
    // Load this core's GDT/TSS (slot cpu_id) and the shared IDT.
    crate::gdt::init(cpu_id);
    crate::idt::load();
    // Enable this core's LAPIC (SVR bit 8; init_ap masks the timer LVT).
    crate::apic::lapic::init_ap(crate::idt::VEC_SPURIOUS);
    // Arm this AP's LAPIC timer in periodic mode with the BSP-calibrated count.
    // `init_ap` left the LVT timer masked; `start_ap_timer` reprograms it UNMASKED
    // with VEC_LAPIC_TIMER at the calibrated count. After this returns, this core
    // receives 100 Hz timer IRQs and `timer_handler` drains PER_CORE_DELAYS[cpu].
    crate::timer::start_ap_timer();
    // Register online. cpu_id() now resolves correctly on this core via the
    // LAPIC ID (mapped by the BSP before bootstrap).
    crate::cpu::mark_online();
    ap_worker_loop()
}

/// AP worker loop: drain pure-CPU jobs from the pool + inter-core inbox messages,
/// then `hlt` until a wake IPI arrives. The BSP sends the wake IPI on every
/// `submit`. Anti-missed-wake: disable IRQs and re-check all queues before
/// sleeping, so a job or message submitted between the drain and the `hlt` is not
/// missed (the `sti; hlt` is atomic — the IPI cannot fire in the 1-instruction
/// shadow of `sti`).
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

        // Run any inter-core messages addressed to this core.
        crate::smp::inbox::drain_inbox(me as u32);

        // No work: sleep until woken, charging the halt as idle.
        x86_64::instructions::interrupts::disable();
        if crate::smp::pool::is_empty() && !crate::smp::inbox::is_pending(me as u32) {
            let idle_start = crate::boot::clock::read_tsc();
            // Atomic sti;hlt — the wake IPI cannot land between the two.
            x86_64::instructions::interrupts::enable_and_hlt();
            crate::sched::cpustat::add_idle(
                me, crate::boot::clock::read_tsc().saturating_sub(idle_start));
        } else {
            // A job or inbox message arrived during the drain/disable window;
            // re-enable and loop.
            x86_64::instructions::interrupts::enable();
        }
    }
}
