//! Application Processor entry point. Limine hands each AP here already in
//! 64-bit long mode on a Limine-owned stack; we load this core's GDT/TSS and
//! the shared IDT, register online, then enter a compute WORKER loop (Fase 2).
//! APs pull pure-CPU jobs from the shared pool and run them on their core;
//! when the queue is empty they PAUSE-spin (no STI/IPI — APs take no
//! interrupts in Fase 2, they busy-poll the job queue).

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
    ap_worker_loop()
}

/// AP worker loop: take pure-CPU jobs from the pool and run them on this core.
/// Spin-waits (PAUSE) when there's no work — no STI/IPI in Fase 2, so the AP
/// polls the queue rather than sleeping on an interrupt.
fn ap_worker_loop() -> ! {
    let me = crate::cpu::cpu_id();
    loop {
        match crate::smp::pool::take() {
            Some(slot) => crate::smp::pool::run_slot(slot, me),
            None => core::hint::spin_loop(),
        }
    }
}
