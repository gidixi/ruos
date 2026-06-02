//! Local APIC: xAPIC MMIO at the per-CPU base. We use it for EOI and the
//! local timer. The base address comes from the MADT.
//!
//! Register offsets (bytes from base, all 32-bit access):
//!   SVR       0xF0  Spurious Interrupt Vector Register
//!   EOI       0xB0  End Of Interrupt
//!   LVT_TIMER 0x320 Local Vector Table for the LAPIC timer
//!   TIMER_INIT  0x380 initial count
//!   TIMER_CUR   0x390 current count
//!   TIMER_DIV   0x3E0 divide configuration

use core::ptr::{read_volatile, write_volatile};

const REG_EOI:        u32 = 0xB0;
const REG_SVR:        u32 = 0xF0;
const REG_ICR_LOW:    u32 = 0x300; // Interrupt Command Register, low dword
const REG_ICR_HIGH:   u32 = 0x310; // Interrupt Command Register, high dword
const REG_LVT_TIMER:  u32 = 0x320;
const REG_TIMER_INIT: u32 = 0x380;
const REG_TIMER_CUR:  u32 = 0x390;
const REG_TIMER_DIV:  u32 = 0x3E0;

const TIMER_MODE_PERIODIC: u32 = 1 << 17;
const TIMER_MASKED:        u32 = 1 << 16;

// ICR fields for an IPI to all processors excluding self.
const ICR_DELIVERY_FIXED:   u32 = 0b000 << 8;  // fixed delivery mode
const ICR_LEVEL_ASSERT:     u32 = 1 << 14;      // assert (vs deassert)
const ICR_DEST_ALL_BUT_SELF: u32 = 0b11 << 18;  // destination shorthand

static mut LAPIC_VIRT: u64 = 0;

fn reg(off: u32) -> *mut u32 {
    // SAFETY: caller ensured `init` ran.
    unsafe { (LAPIC_VIRT + off as u64) as *mut u32 }
}

pub fn init(phys_base: u64, spurious_vector: u8) {
    let virt = crate::memory::map_io_page(x86_64::PhysAddr::new(phys_base))
        .expect("lapic mmio map");
    // SAFETY: single-threaded boot, no other writers to LAPIC_VIRT.
    unsafe {
        LAPIC_VIRT = virt.as_u64();
        // Enable LAPIC: set bit 8 in SVR, OR in the spurious vector.
        let cur = read_volatile(reg(REG_SVR));
        write_volatile(reg(REG_SVR), cur | (1 << 8) | spurious_vector as u32);
        // Divide config = 16.
        write_volatile(reg(REG_TIMER_DIV), 0x3);
        // Mask the timer until configured.
        write_volatile(reg(REG_LVT_TIMER), TIMER_MASKED);
    }
}

pub fn eoi() {
    // SAFETY: init ran; EOI is always safe to write.
    unsafe { write_volatile(reg(REG_EOI), 0) };
}

/// Minimal per-AP LAPIC setup. The BSP already mapped `LAPIC_VIRT` (shared MMIO
/// base — the LAPIC is per-core but the register window is the same address on
/// every core). Each AP must still enable ITS OWN LAPIC (SVR bit 8) and mask
/// its timer LVT before enabling interrupts, or delivered IPIs (the wake) may
/// not be serviced. Does NOT remap or recalibrate (BSP-only).
pub fn init_ap(spurious_vector: u8) {
    // SAFETY: LAPIC_VIRT was set by the BSP's init() before any AP starts; the
    // register window is identical per core.
    unsafe {
        let cur = read_volatile(reg(REG_SVR));
        write_volatile(reg(REG_SVR), cur | (1 << 8) | spurious_vector as u32);
        write_volatile(reg(REG_LVT_TIMER), TIMER_MASKED);
    }
}

/// Send an inter-processor interrupt with `vector` to all processors except
/// the calling one (destination shorthand "all excluding self"). Used by the
/// BSP to wake sleeping AP worker cores after submitting a job.
pub fn send_ipi_all_but_self(vector: u8) {
    let low = ICR_DEST_ALL_BUT_SELF | ICR_LEVEL_ASSERT | ICR_DELIVERY_FIXED | vector as u32;
    // SAFETY: init ran. Write high (dest field unused for the shorthand) then
    // low — writing the low dword dispatches the IPI.
    unsafe {
        write_volatile(reg(REG_ICR_HIGH), 0);
        write_volatile(reg(REG_ICR_LOW), low);
    }
}

/// Calibrate the LAPIC timer: run it at max count for `ms` milliseconds measured
/// via the **TSC-based boot clock** and return the LAPIC ticks elapsed.
///
/// Uses the TSC (calibrated from CPUID in `boot::clock::init`, which runs before
/// the interrupts phase) instead of the PIT: real UEFI machines often gate the
/// PIT off, and polling it then hangs the boot forever. The busy-wait is bounded
/// by the monotonic boot clock.
pub fn calibrate(ms: u32) -> u32 {
    // TSC-cycle-precise window (elapsed_ms has 1ms granularity → ~10% error
    // over a 10ms window). tsc_per_ms is set in boot::clock::init (CPUID or
    // bounded-PIT fallback), always nonzero by the interrupts phase.
    let target = crate::boot::clock::tsc_per_ms().saturating_mul(ms as u64);
    // SAFETY: init ran; LAPIC timer registers are valid.
    unsafe {
        // Start LAPIC timer at max count, masked one-shot.
        write_volatile(reg(REG_LVT_TIMER), TIMER_MASKED);
        write_volatile(reg(REG_TIMER_INIT), 0xFFFF_FFFF);

        let t0 = crate::boot::clock::read_tsc();
        while crate::boot::clock::read_tsc().wrapping_sub(t0) < target {
            core::hint::spin_loop();
        }

        let remaining = read_volatile(reg(REG_TIMER_CUR));
        write_volatile(reg(REG_TIMER_INIT), 0); // stop
        0xFFFF_FFFF - remaining
    }
}

pub fn set_timer_periodic(vector: u8, initial_count: u32) {
    // SAFETY: init ran.
    unsafe {
        write_volatile(reg(REG_LVT_TIMER), TIMER_MODE_PERIODIC | vector as u32);
        write_volatile(reg(REG_TIMER_INIT), initial_count);
    }
}

/// Local APIC ID of the current core (xAPIC: register 0x20, bits 31:24).
pub fn apic_id() -> u32 {
    // SAFETY: init ran before this is ever called; reg(0x20) is the ID register.
    let raw = unsafe { read_volatile(reg(0x20)) };
    raw >> 24
}
