//! Phase 1 — architectural state: GDT/TSS + IDT.

use crate::boot::BootError;

pub fn init() -> Result<(), BootError> {
    crate::gdt::init(0); // BSP = slot 0
    crate::idt::init();
    crate::binfo!("arch", "GDT/TSS + IDT up");

    // Smoke: software breakpoint — INT3 is handled by the IDT, not maskable
    // by IF, so we can test it before STI. Gated by feature to keep default
    // output clean.
    #[cfg(feature = "boot-checks")]
    {
        unsafe { core::arch::asm!("int3") };
        crate::binfo!("arch", "INT3 smoke ok");
    }

    Ok(())
}
