//! Boot clock — TSC-based elapsed-time source for the boot logger.
//!
//! Pre-LAPIC-init, `timer::ticks()` returns 0 (timer hasn't fired yet),
//! so every boot log line stamps `T+0.000s`. We want sub-second
//! granularity from the very first log line, so we sample `rdtsc` at
//! `init()` (called from kmain right after serial up) and convert TSC
//! deltas to ms via a PIT-calibrated frequency.
//!
//! TSC frequency is calibrated by polling PIT channel 2 for ~10 ms and
//! computing `ticks / 0.010s = freq`. Takes one PIT round-trip at boot.

use core::sync::atomic::{AtomicU64, Ordering};

static BOOT_TSC: AtomicU64 = AtomicU64::new(0);
static TSC_PER_MS: AtomicU64 = AtomicU64::new(0);

/// Read the TSC. Inline asm; non-serializing (good enough for our
/// millisecond-granularity logger).
#[inline(always)]
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Calibrate TSC against PIT channel 2 by measuring TSC delta over 10 ms.
fn calibrate_tsc_per_ms() -> u64 {
    use x86_64::instructions::port::Port;
    // Enable PIT channel 2 (speaker gate, but we only read counter).
    let mut port_61: Port<u8> = Port::new(0x61);
    let mut pit_mode: Port<u8> = Port::new(0x43);
    let mut pit_ch2: Port<u8> = Port::new(0x42);

    unsafe {
        // Disable speaker, enable gate (bit 0 = gate on for ch2).
        let v = port_61.read();
        port_61.write((v & !0x02) | 0x01);

        // Mode 0 (one-shot), binary, ch2.
        pit_mode.write(0xB0);

        // Load counter = 1193182 * 10 / 1000 = 11932 (10 ms at 1.193182 MHz).
        let initial: u16 = 11932;
        pit_ch2.write((initial & 0xFF) as u8);
        pit_ch2.write((initial >> 8) as u8);

        let tsc_start = rdtsc();
        // Wait until PIT bit 5 of port 0x61 is set (one-shot expired).
        while (port_61.read() & 0x20) == 0 {}
        let tsc_end = rdtsc();

        (tsc_end - tsc_start) / 10  // = TSC per 1 ms
    }
}

/// Sample boot TSC and calibrate. Must be called once from kmain
/// before any boot::log emit.
pub fn init() {
    let freq = calibrate_tsc_per_ms();
    TSC_PER_MS.store(freq, Ordering::Release);
    // BOOT_TSC stamped *after* calibration; we want elapsed_ms() to
    // start from 0 at the first post-init log, not the calibrate point.
    BOOT_TSC.store(rdtsc(), Ordering::Release);
}

/// Milliseconds since `init()`.
pub fn elapsed_ms() -> u64 {
    let freq = TSC_PER_MS.load(Ordering::Acquire);
    if freq == 0 { return 0; }
    let boot = BOOT_TSC.load(Ordering::Acquire);
    let now = rdtsc();
    now.saturating_sub(boot) / freq
}
