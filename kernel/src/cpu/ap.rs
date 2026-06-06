//! Application Processor entry point. Limine hands each AP here already in
//! 64-bit long mode on a Limine-owned stack; we load this core's GDT/TSS and
//! the shared IDT, enable this core's LAPIC, arm this core's LAPIC timer in
//! periodic mode (100 Hz, same calibrated count as the BSP), register online,
//! then enter its per-core cooperative executor (Step 3b). The executor loop
//! polls async tasks, drains the inter-core inbox, drains the compute pool
//! (so banded compositing keeps its workers), and `hlt`s when idle. The timer
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
    // Step 5: dispatch on the role assigned to this core in smp::bringup.
    // GuiCompositor → dedicated GUI spinner (waits for compositor hand-off, then
    //   runs run_compositor_gate forever — never returns to the executor).
    // All others → per-core cooperative executor (Step 3b), which drains async
    //   tasks, the inbox, and the compute pool (banded compositing workers).
    match crate::cpu::core_role(cpu_id as u32) {
        crate::cpu::CoreRole::GuiCompositor =>
            crate::wasm::wt::wm::gui_worker_loop(),
        _ =>
            crate::executor::run_core(cpu_id as u32),
    }
}
