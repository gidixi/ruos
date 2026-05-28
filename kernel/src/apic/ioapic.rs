//! I/O APIC: legacy ISA IRQs are routed here. We translate `irq` through
//! ACPI's IRQ source overrides, then write a 64-bit redirection table entry
//! that fires the requested IDT vector.

use core::ptr::{read_volatile, write_volatile};
use crate::acpi_init::IrqOverride;

const REG_IOREDTBL_BASE: u32 = 0x10;

static mut IOAPIC_VIRT: u64 = 0;

fn ioregsel() -> *mut u32 { unsafe { IOAPIC_VIRT as *mut u32 } }
fn iowin()    -> *mut u32 { unsafe { (IOAPIC_VIRT + 0x10) as *mut u32 } }

fn read(idx: u32) -> u32 {
    // SAFETY: init ran.
    unsafe {
        write_volatile(ioregsel(), idx);
        read_volatile(iowin())
    }
}

fn write(idx: u32, val: u32) {
    // SAFETY: init ran.
    unsafe {
        write_volatile(ioregsel(), idx);
        write_volatile(iowin(), val);
    }
}

pub fn init(phys_base: u64, hhdm_offset: u64) {
    // Limine's HHDM does not cover IOAPIC MMIO — map it explicitly as UC.
    crate::apic::mmio::map_mmio_page(phys_base, hhdm_offset);
    // SAFETY: single-threaded boot.
    unsafe { IOAPIC_VIRT = phys_base + hhdm_offset; }

    // Read max redirection entry from IOAPICVER (index 0x01, bits 16..23).
    let ver = read(0x01);
    let max_redir = ((ver >> 16) & 0xFF) as u32; // count is max+1

    // Mask everything until explicit redirect() calls.
    for i in 0..=max_redir {
        let idx = REG_IOREDTBL_BASE + i * 2;
        write(idx, 1 << 16);     // masked
        write(idx + 1, 0);       // destination APIC id 0
    }
}

fn translate(irq: u8, overrides: &[IrqOverride]) -> (u32, bool, bool) {
    for o in overrides {
        if o.source == irq {
            return (o.global_system_interrupt, o.active_low, o.level_triggered);
        }
    }
    (irq as u32, false, false) // identity: active high + edge
}

pub fn redirect(irq: u8, vector: u8, overrides: &[IrqOverride]) {
    let (gsi, active_low, level) = translate(irq, overrides);
    let idx = REG_IOREDTBL_BASE + gsi * 2;
    let mut low = vector as u32;       // delivery mode 0 (fixed), phys dest, unmasked
    if active_low { low |= 1 << 13; }
    if level      { low |= 1 << 15; }
    write(idx, low);
    write(idx + 1, 0);                  // destination APIC id 0
}
