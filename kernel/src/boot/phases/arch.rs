//! Phase 1 — architectural state: GDT/TSS + IDT.

use crate::boot::BootError;

pub fn init() -> Result<(), BootError> {
    crate::gdt::init(0); // BSP = slot 0
    crate::idt::init();
    crate::binfo!("arch", "GDT/TSS + IDT up");
    // Enable SSE/AVX AFTER the IDT so any #GP here is reported, not a triple fault.
    enable_simd();
    crate::binfo!("arch", "SIMD enabled");
    // PAT: Limine already programs the BSP, but set it here too so every core's
    // PAT layout comes from the same kernel-owned place (APs do it in ap_entry).
    crate::cpu::init_pat();
    crate::binfo!("arch", "PAT programmed (PA5=WC)");

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

/// Enable SSE (always) and AVX (if the CPU supports XSAVE+AVX) so AOT cranelift
/// code can use them. The integer-only kernel never needed SIMD, so it was left
/// disabled. cranelift's SSE-only float codegen renders egui text garbled in
/// ruos (works on PC where AVX is used) → enable AVX to match the working path.
pub(crate) fn enable_simd() {
    use x86_64::registers::control::{Cr0, Cr0Flags, Cr4, Cr4Flags};
    unsafe {
        let mut cr0 = Cr0::read();
        cr0.remove(Cr0Flags::EMULATE_COPROCESSOR);
        cr0.insert(Cr0Flags::MONITOR_COPROCESSOR);
        Cr0::write(cr0);

        // CPUID leaf 1: ECX bit26 = XSAVE, bit28 = AVX.
        let ecx: u32;
        core::arch::asm!(
            "push rbx", "mov eax, 1", "cpuid", "mov {e:e}, ecx", "pop rbx",
            e = out(reg) ecx, out("eax") _, out("ecx") _, out("edx") _,
            options(nostack),
        );
        let has_xsave = ecx & (1 << 26) != 0;
        let has_avx = ecx & (1 << 28) != 0;

        let mut cr4 = Cr4::read();
        cr4.insert(Cr4Flags::OSFXSR | Cr4Flags::OSXMMEXCPT_ENABLE);
        if has_xsave && has_avx {
            cr4.insert(Cr4Flags::OSXSAVE);
        }
        Cr4::write(cr4);

        if has_xsave && has_avx {
            // XCR0 = X87 | SSE | AVX (bits 0,1,2). Requires CR4.OSXSAVE (set above).
            core::arch::asm!(
                "xsetbv",
                in("ecx") 0u32, in("eax") 0b111u32, in("edx") 0u32,
                options(nostack),
            );
        }

        // MXCSR to a known IEEE state (round-nearest, masked, FTZ/DAZ off).
        let mxcsr: u32 = 0x0000_1F80;
        core::arch::asm!(
            "fninit",
            "ldmxcsr [{m}]",
            m = in(reg) &mxcsr,
            options(nostack),
        );
    }
}
