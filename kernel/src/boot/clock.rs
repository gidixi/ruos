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
pub fn read_tsc() -> u64 {
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

/// Busy-wait `us` microseconds on the TSC. Bounded by construction (a TSC
/// deadline). For synchronous driver bring-up paths where the async executor /
/// `Delay` future is unusable (e.g. USB-WiFi RF init run inside enumeration).
pub fn udelay(us: u64) {
    let cycles = us.saturating_mul(tsc_per_ms()) / 1000;
    let start = read_tsc();
    while read_tsc().wrapping_sub(start) < cycles {
        core::hint::spin_loop();
    }
}

/// Busy-wait `ms` milliseconds on the TSC (e.g. RADIO_A table msleep markers).
pub fn mdelay(ms: u64) {
    let cycles = ms.saturating_mul(tsc_per_ms());
    let start = read_tsc();
    while read_tsc().wrapping_sub(start) < cycles {
        core::hint::spin_loop();
    }
}

/// TSC ticks per millisecond. Tries CPUID first (works without the PIT, which
/// real UEFI machines often gate off — polling it then hangs forever), then a
/// **bounded** PIT measurement, then a safe default. NEVER hangs.
fn calibrate_tsc_per_ms() -> u64 {
    if let Some(hz) = tsc_hz_from_cpuid() {
        return hz / 1000;
    }
    if let Some(per_ms) = calibrate_pit_bounded() {
        return per_ms;
    }
    // Last resort: assume 2 GHz. The boot clock is then approximate — only
    // affects log timestamps + bounded-wait scaling, never correctness.
    crate::kprintln!("ruos: TSC calibration fell back to 2GHz default");
    2_000_000
}

/// TSC frequency (Hz) from CPUID, or None if the CPU doesn't report it.
/// Leaf 0x15: TSC = crystal_hz * ratio_num / ratio_den. Leaf 0x16: base MHz.
fn tsc_hz_from_cpuid() -> Option<u64> {
    use core::arch::x86_64::__cpuid;
    // SAFETY: CPUID is always available on x86-64; leaf 0 gives the max leaf.
    let max_leaf = unsafe { __cpuid(0) }.eax;
    if max_leaf >= 0x15 {
        let r = unsafe { __cpuid(0x15) };
        // eax = ratio denominator, ebx = numerator, ecx = core crystal Hz.
        if r.eax != 0 && r.ebx != 0 && r.ecx != 0 {
            return Some((r.ecx as u64) * (r.ebx as u64) / (r.eax as u64));
        }
    }
    if max_leaf >= 0x16 {
        let r = unsafe { __cpuid(0x16) };
        // eax = base frequency in MHz (TSC runs at base freq on modern Intel).
        if r.eax != 0 {
            return Some((r.eax as u64) * 1_000_000);
        }
    }
    None
}

/// Bounded PIT-channel-2 measurement of TSC ticks/ms. Returns None if the PIT
/// one-shot never signals within a generous TSC-cycle cap (i.e. the PIT is
/// dead/gated) instead of spinning forever.
fn calibrate_pit_bounded() -> Option<u64> {
    use x86_64::instructions::port::Port;
    let mut port_61: Port<u8> = Port::new(0x61);
    let mut pit_mode: Port<u8> = Port::new(0x43);
    let mut pit_ch2: Port<u8> = Port::new(0x42);
    // Cap the spin: 10ms even at a 100 GHz TSC is 1e9 cycles; 8e9 is a huge
    // margin that still bounds the loop on a dead PIT (~a couple seconds max).
    const CYCLE_CAP: u64 = 8_000_000_000;
    unsafe {
        let v = port_61.read();
        port_61.write((v & !0x02) | 0x01); // speaker off, ch2 gate on
        pit_mode.write(0xB0);              // ch2, lobyte/hibyte, mode 0 (one-shot)
        let initial: u16 = 11932;          // 10 ms at 1.193182 MHz
        pit_ch2.write((initial & 0xFF) as u8);
        pit_ch2.write((initial >> 8) as u8);

        let tsc_start = read_tsc();
        loop {
            if port_61.read() & 0x20 != 0 { break; } // one-shot expired (OUT high)
            if read_tsc().wrapping_sub(tsc_start) > CYCLE_CAP {
                return None; // PIT never fired — dead/gated; don't hang
            }
            core::hint::spin_loop();
        }
        let tsc_end = read_tsc();
        Some((tsc_end - tsc_start) / 10)
    }
}

/// Sample boot TSC and calibrate. Must be called once from kmain
/// before any boot::log emit.
pub fn init() {
    let freq = calibrate_tsc_per_ms();
    TSC_PER_MS.store(freq, Ordering::Release);
    // BOOT_TSC stamped *after* calibration; we want elapsed_ms() to
    // start from 0 at the first post-init log, not the calibrate point.
    BOOT_TSC.store(read_tsc(), Ordering::Release);
}

/// Milliseconds since `init()`.
pub fn elapsed_ms() -> u64 {
    let freq = TSC_PER_MS.load(Ordering::Acquire);
    if freq == 0 { return 0; }
    let boot = BOOT_TSC.load(Ordering::Acquire);
    let now = read_tsc();
    now.saturating_sub(boot) / freq
}

/// Calibrated TSC ticks per millisecond (0 until `init()` runs).
pub fn tsc_per_ms() -> u64 {
    TSC_PER_MS.load(Ordering::Acquire)
}

/// Overwrite the TSC/ms calibration. Called from the interrupts phase once the
/// ACPI PM timer (an accurate fixed-frequency reference) is available, to correct
/// the rough CPUID/PIT estimate made at `init()` time (before ACPI was parsed).
pub fn set_tsc_per_ms(v: u64) {
    if v != 0 { TSC_PER_MS.store(v, Ordering::Release); }
}
