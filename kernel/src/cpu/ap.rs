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
    // Program IA32_PAT on THIS core. PAT is per-core: the BSP inherits Limine's
    // layout (index 5 = WC, used by the framebuffer mapping) but an AP boots
    // with the reset default where index 5 = WT. Compute/GUI cores blit to the
    // framebuffer, so without this every present from an AP writes VRAM
    // write-through (uncombined) — fluid in VMs, slideshow on real hardware.
    // Must precede any framebuffer access on this core.
    crate::cpu::init_pat();
    // Enable SSE/AVX on THIS core. CR0/CR4 SIMD-enable bits are PER-CPU; the BSP
    // set them in `arch::init` but each AP boots with SIMD disabled (CR0.EM set).
    // Without this, the first SSE instruction in AOT cranelift float code (e.g.
    // egui rendering on the GUI core) faults with #UD and the core dies. Mirror
    // the BSP exactly. Done AFTER the IDT so any #GP here is reported, not a triple
    // fault.
    crate::boot::phases::arch::enable_simd();
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
    // Limine hands each AP a SMALL stack (~64 KiB default); the BSP runs on a
    // 16 MiB stack (`limine.conf` `stack_size: 0x1000000`). The GUI core runs the
    // egui compositor via `run_cwasm`, whose render call chain (egui PassState /
    // tessellation) is deep enough to overflow the tiny Limine AP stack → silent
    // memory corruption → a bad jump → `#UD` mid-instruction (observed on the GUI
    // core only; the BSP, with its 16 MiB stack, runs the same compositor fine).
    // Compute APs run `run_cwasm` for exec'd apps too, so give EVERY AP a large
    // heap-backed stack and switch to it before entering the worker (which never
    // returns — the leaked stack lives for the core's lifetime).
    const AP_STACK_SIZE: usize = 8 * 1024 * 1024; // 8 MiB, generous headroom
    let stack = alloc::vec![0u8; AP_STACK_SIZE].leak();
    // 16-byte aligned top; SysV requires RSP%16==0 before a `call`.
    let top = (stack.as_mut_ptr() as u64 + AP_STACK_SIZE as u64) & !0xF;
    core::arch::asm!(
        "mov rsp, {top}",
        "call {run}",
        top = in(reg) top,
        run = in(reg) ap_run as unsafe extern "C" fn(u64) -> !,
        in("rdi") cpu_id as u64,   // SysV first arg → ap_run(cpu_id)
        options(noreturn),
    );
}

/// AP worker, entered on the large heap stack set up by `ap_entry`.
/// Dispatches on this core's role; neither arm returns.
///
/// GuiCompositor → dedicated GUI spinner (waits for compositor hand-off, then
///   runs run_compositor_gate forever). All others → per-core cooperative
///   executor (Step 3b): async tasks, inbox, compute pool (banded compositing).
unsafe extern "C" fn ap_run(cpu_id: u64) -> ! {
    match crate::cpu::core_role(cpu_id as u32) {
        crate::cpu::CoreRole::GuiCompositor =>
            crate::wasm::wt::wm::gui_worker_loop(),
        _ =>
            crate::executor::run_core(cpu_id as u32),
    }
}
