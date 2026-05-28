//! LAPIC timer driver. Calibrates LAPIC frequency via PIT, configures the
//! timer in periodic mode at the requested frequency, and exposes a tick
//! counter consumed by `kmain` for the boot smoke test.

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::structures::idt::InterruptStackFrame;
use crate::{apic::lapic, idt, kprintln};

pub static TICKS: AtomicU64 = AtomicU64::new(0);

pub extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    lapic::eoi();
}

pub fn ticks() -> u64 { TICKS.load(Ordering::Relaxed) }

pub fn init(hz: u32) -> Result<(), &'static str> {
    // Calibrate over 10 ms (100 PIT samples per second).
    let lapic_per_10ms = lapic::calibrate(10);
    if lapic_per_10ms == 0 { return Err("calibration"); }
    let lapic_per_sec = lapic_per_10ms * 100;
    let initial_count = lapic_per_sec / hz;
    if initial_count == 0 { return Err("hz too high"); }

    kprintln!(
        "ruos: lapic calibrated {} ticks/sec, periodic count={}",
        lapic_per_sec, initial_count
    );

    lapic::set_timer_periodic(idt::VEC_LAPIC_TIMER, initial_count);
    Ok(())
}
