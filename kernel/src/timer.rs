//! LAPIC timer driver. Calibrates LAPIC frequency, configures the timer in
//! periodic mode at the requested frequency, and exposes a tick counter
//! consumed by `kmain` for the boot smoke test. Per-core aware: only the BSP
//! (cpu==0) increments the global `TICKS` wall clock (spec inv. 8); APs read
//! it. Every core drains its own `PER_CORE_DELAYS` list on each timer fire.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use x86_64::structures::idt::InterruptStackFrame;
use crate::{apic::lapic, idt};

/// Global wall-clock tick counter. **Only the BSP (cpu==0) increments this**
/// (single-writer invariant, spec inv. 8). APs read it via `ticks()`.
pub static TICKS: AtomicU64 = AtomicU64::new(0);

/// Per-AP tick counters (indices 1..MAX_CPUS). Bumped by `timer_handler` on
/// every AP timer fire (cpu > 0). Index 0 is unused (BSP). Used only in the
/// boot-check gate to verify the AP LAPIC timer is actually firing.
static AP_TICKS: [AtomicU64; crate::cpu::MAX_CPUS] = {
    const Z: AtomicU64 = AtomicU64::new(0);
    [Z; crate::cpu::MAX_CPUS]
};

/// The calibrated LAPIC periodic count published by `init()` so that APs can
/// arm their own timers with the same value via `start_ap_timer()`.
static AP_TIMER_COUNT: AtomicU32 = AtomicU32::new(0);

pub extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    let cpu = crate::cpu::cpu_id();
    // Guard: during `probe_fast_cpuid()` the BSP's TSC_AUX is briefly set to
    // the probe sentinel 0xABCD before being restored to 0. If a timer IRQ
    // lands in that window, cpu_id() returns an out-of-range value. Treat any
    // cpu >= MAX_CPUS as the BSP (cpu==0) for the purposes of this handler --
    // the caller restores TSC_AUX immediately after, so this window is <1 us.
    let cpu = if (cpu as usize) < crate::cpu::MAX_CPUS { cpu } else { 0 };
    let now = if cpu == 0 {
        // BSP: advance the global wall clock and tick the framebuffer cursor.
        // fetch_add returns the *previous* value, so add 1 to get "now".
        let n = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
        crate::console::fb::tick_cursor();
        n
    } else {
        // APs: read the shared clock (never increment it -- single-writer inv. 8).
        // Also bump this core's per-AP tick counter for the boot-check gate.
        AP_TICKS[cpu as usize].fetch_add(1, Ordering::Relaxed);
        TICKS.load(Ordering::Relaxed)
    };
    crate::executor::delay::timer_tick_core(now, cpu);
    lapic::eoi();
}

/// Read the global wall-clock tick counter (BSP-managed; APs read-only).
pub fn ticks() -> u64 { TICKS.load(Ordering::Relaxed) }

/// Read the per-AP tick counter for `cpu` (cpu > 0 meaningful; cpu==0 always 0).
/// Used by the boot-check gate to verify an AP's LAPIC timer is firing.
pub fn ap_ticks(cpu: u32) -> u64 {
    AP_TICKS[cpu as usize].load(Ordering::Relaxed)
}

/// Arm THIS (AP) core's LAPIC timer in periodic mode with the count the BSP
/// calibrated in `init()`. No-op (returns) if calibration has not run yet.
///
/// Call in `ap_entry` AFTER `lapic::init_ap` and BEFORE entering the worker
/// loop. `init_ap` masks the LVT timer; this function reprograms it UNMASKED
/// with `VEC_LAPIC_TIMER` at the calibrated periodic count.
pub fn start_ap_timer() {
    let count = AP_TIMER_COUNT.load(Ordering::SeqCst);
    if count == 0 { return; }
    lapic::set_timer_periodic(idt::VEC_LAPIC_TIMER, count);
}

pub fn init(hz: u32, pm_timer: Option<(u16, bool)>) -> Result<(), &'static str> {
    // Calibrate the LAPIC over a 10 ms window. Prefer the ACPI PM timer (accurate
    // even when the PIT is gated off); else fall back to the TSC.
    let lapic_per_10ms = lapic::calibrate(10, pm_timer);
    if lapic_per_10ms == 0 { return Err("calibration"); }
    // u64 to avoid overflow on >4.29 GHz LAPIC buses.
    let lapic_per_sec = (lapic_per_10ms as u64) * 100;
    let initial_count_u64 = lapic_per_sec / hz as u64;
    if initial_count_u64 == 0 { return Err("hz too high"); }
    if initial_count_u64 > u32::MAX as u64 { return Err("hz too low"); }
    let initial_count = initial_count_u64 as u32;

    crate::binfo!("intr", "lapic calibrated {} ticks/sec, periodic count={}", lapic_per_sec, initial_count);

    // Publish the count BEFORE arming the BSP timer so start_ap_timer() on APs
    // (which run after init()) always sees a non-zero value.
    AP_TIMER_COUNT.store(initial_count, Ordering::SeqCst);
    lapic::set_timer_periodic(idt::VEC_LAPIC_TIMER, initial_count);
    Ok(())
}
