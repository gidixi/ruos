//! Power management: reboot and poweroff.
//!
//! Reboot: pulse the keyboard controller (port 0x64, command 0xFE).
//! Universally supported on x86 — qemu, vbox, real hardware. Falls
//! through to triple-fault if the controller doesn't respond.
//!
//! Poweroff: try the well-known I/O debug-exit ports in sequence —
//!   QEMU isa-debug-exit at 0x604
//!   VirtualBox at 0x4004
//!   QEMU q35 ACPI shutdown at 0xB004
//! If none respond, halt forever. ACPI S5 sleep (proper poweroff via
//! FADT + DSDT _S5 SLP_TYPa) deferred — would need AML parser.

use x86_64::instructions::port::Port;
use x86_64::instructions::interrupts;

/// Reboot the system. Never returns.
pub fn reboot() -> ! {
    interrupts::disable();
    let mut cmd: Port<u8> = Port::new(0x64);
    // Wait for keyboard input buffer to drain, then issue reset cmd 0xFE.
    for _ in 0..1024 {
        unsafe {
            if cmd.read() & 0x02 == 0 {
                cmd.write(0xFE);
            }
        }
        for _ in 0..10_000 { core::hint::spin_loop(); }
    }
    // Keyboard controller didn't reset — triple-fault by loading null IDT.
    unsafe {
        let null_idt = x86_64::structures::DescriptorTablePointer {
            limit: 0,
            base: x86_64::VirtAddr::new(0),
        };
        x86_64::instructions::tables::lidt(&null_idt);
        core::arch::asm!("int3");
    }
    loop { x86_64::instructions::hlt(); }
}

/// Power off the system. Never returns.
pub fn poweroff() -> ! {
    interrupts::disable();
    unsafe {
        // QEMU isa-debug-exit (works if -device isa-debug-exit set).
        let mut p604: Port<u16> = Port::new(0x604);
        p604.write(0x2000);
        // VirtualBox.
        let mut p4004: Port<u16> = Port::new(0x4004);
        p4004.write(0x3400);
        // QEMU q35 ACPI shutdown.
        let mut pb004: Port<u16> = Port::new(0xB004);
        pb004.write(0x2000);
    }
    // Nothing worked — halt.
    loop { x86_64::instructions::hlt(); }
}
