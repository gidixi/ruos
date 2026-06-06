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

/// Send a fixed IPI with `vector` to a SINGLE target core, addressed by its
/// xAPIC `lapic_id` (physical destination mode). Dest-shorthand bits 18-19
/// are 0 (no shorthand) so the destination comes from ICR_HIGH bits 24-31.
/// Used for targeted cross-core wake (VEC_WAKE) and inbox delivery (VEC_INBOX).
pub fn send_ipi(lapic_id: u32, vector: u8) {
    // No shorthand (bits 18-19 = 0) → use physical destination in ICR_HIGH.
    let low = ICR_DELIVERY_FIXED | ICR_LEVEL_ASSERT | vector as u32;
    // SAFETY: init ran. Write HIGH first (sets dest), then LOW (dispatches IPI).
    unsafe {
        write_volatile(reg(REG_ICR_HIGH), lapic_id << 24); // dest in bits 24-31 (xAPIC)
        write_volatile(reg(REG_ICR_LOW), low);
    }
}

/// Calibrate the LAPIC timer: run it at max count for `ms` milliseconds measured
/// LAPIC ticks elapsed, measuring the `ms` window against the best reference.
///
/// `pm_timer = Some((port, is_32bit))` → use the ACPI **PM timer** (fixed
/// 3.579545 MHz, accurate even when the PIT is gated off — the real-UEFI case)
/// and ALSO recalibrate the boot-clock TSC from the same window. `None` → fall
/// back to the TSC (itself from CPUID), which on some real hardware over-reports
/// and makes the 100 Hz timer run slow. Never uses the PIT (it hangs on UEFI).
pub fn calibrate(ms: u32, pm_timer: Option<(u16, bool)>) -> u32 {
    use x86_64::instructions::port::Port;
    const PM_FREQ: u64 = 3_579_545; // ACPI PM timer, fixed

    // SAFETY: init ran; LAPIC timer registers are valid.
    unsafe {
        // Start LAPIC timer at max count, masked one-shot.
        write_volatile(reg(REG_LVT_TIMER), TIMER_MASKED);
        write_volatile(reg(REG_TIMER_INIT), 0xFFFF_FFFF);
        let t0 = crate::boot::clock::read_tsc();

        match pm_timer {
            Some((port, is_32bit)) => {
                let mask: u32 = if is_32bit { 0xFFFF_FFFF } else { 0x00FF_FFFF };
                let pm_ticks = (PM_FREQ * ms as u64 / 1000) as u32; // ticks for `ms` ms
                let mut p: Port<u32> = Port::new(port);
                let start = p.read() & mask;
                loop {
                    let elapsed = p.read().wrapping_sub(start) & mask;
                    if elapsed >= pm_ticks { break; }
                    core::hint::spin_loop();
                }
            }
            None => {
                let target = crate::boot::clock::tsc_per_ms().saturating_mul(ms as u64);
                while crate::boot::clock::read_tsc().wrapping_sub(t0) < target {
                    core::hint::spin_loop();
                }
            }
        }

        let t1 = crate::boot::clock::read_tsc();
        let remaining = read_volatile(reg(REG_TIMER_CUR));
        write_volatile(reg(REG_TIMER_INIT), 0); // stop

        // If we used the accurate PM timer, correct the boot-clock TSC too.
        if pm_timer.is_some() && ms > 0 {
            crate::boot::clock::set_tsc_per_ms(t1.wrapping_sub(t0) / ms as u64);
        }
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
/// Returns 0 (BSP sentinel) if `init` has not run yet — safe for very early boot
/// code (e.g. the magazine allocator cpu_id path) where the MMIO window is not
/// yet mapped.
pub fn apic_id() -> u32 {
    // SAFETY: read is atomic on x86 (naturally aligned 64-bit on 64-bit kernel).
    if unsafe { core::ptr::read_volatile(&LAPIC_VIRT) } == 0 {
        return 0;   // pre-init: MMIO not mapped; return BSP sentinel
    }
    // SAFETY: LAPIC_VIRT != 0 means init ran; reg(0x20) is the ID register.
    let raw = unsafe { read_volatile(reg(0x20)) };
    raw >> 24
}
