//! Phase 1 — architectural state: GDT/TSS + IDT.

use crate::boot::BootError;

pub fn init() -> Result<(), BootError> {
    enable_sse();
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

/// Enable SSE so AOT-compiled WASM code (cranelift emits SSE/SSE2..SSE4.2 for
/// float/vector ops) doesn't #UD. The integer-only kernel never needed it, so it
/// was left disabled. Clears CR0.EM, sets CR0.MP, and CR4.OSFXSR + OSXMMEXCPT.
/// (No AVX: the AOT modules are pinned to an SSE4.2 baseline.)
fn enable_sse() {
    use x86_64::registers::control::{Cr0, Cr0Flags, Cr4, Cr4Flags};
    unsafe {
        let mut cr0 = Cr0::read();
        cr0.remove(Cr0Flags::EMULATE_COPROCESSOR);
        cr0.insert(Cr0Flags::MONITOR_COPROCESSOR);
        Cr0::write(cr0);
        let mut cr4 = Cr4::read();
        cr4.insert(Cr4Flags::OSFXSR | Cr4Flags::OSXMMEXCPT_ENABLE);
        Cr4::write(cr4);
    }
}
