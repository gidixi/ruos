//! Interrupt Descriptor Table and CPU exception handlers.
//!
//! Hardware IRQ vectors (timer, keyboard) are declared as constants here and
//! the handlers themselves live in `timer.rs` / `keyboard.rs`.

use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use x86_64::VirtAddr;
use crate::{kprintln, gdt};

pub const VEC_LAPIC_TIMER: u8 = 0x20;
pub const VEC_KEYBOARD:    u8 = 0x21;
pub const VEC_SPURIOUS:    u8 = 0xFF;

static IDT: spin::Once<InterruptDescriptorTable> = spin::Once::new();

pub fn init() {
    let idt = IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();

        idt.divide_error.set_handler_fn(de_handler);
        idt.invalid_opcode.set_handler_fn(ud_handler);
        idt.general_protection_fault.set_handler_fn(gp_handler);
        idt.page_fault.set_handler_fn(pf_handler);
        // SAFETY: stack index 0 is configured in gdt.rs.
        unsafe {
            idt.double_fault
                .set_handler_fn(df_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt.breakpoint.set_handler_fn(bp_handler);

        idt
    });
    idt.load();
}

extern "x86-interrupt" fn de_handler(frame: InterruptStackFrame) {
    kprintln!("ruos: #DE at rip=0x{:X}", frame.instruction_pointer.as_u64());
    halt();
}

extern "x86-interrupt" fn ud_handler(frame: InterruptStackFrame) {
    kprintln!("ruos: #UD at rip=0x{:X}", frame.instruction_pointer.as_u64());
    halt();
}

extern "x86-interrupt" fn gp_handler(frame: InterruptStackFrame, code: u64) {
    kprintln!(
        "ruos: #GP rip=0x{:X} err=0x{:X}",
        frame.instruction_pointer.as_u64(), code
    );
    halt();
}

extern "x86-interrupt" fn pf_handler(frame: InterruptStackFrame, code: PageFaultErrorCode) {
    let cr2 = x86_64::registers::control::Cr2::read().unwrap_or(VirtAddr::zero());
    kprintln!(
        "ruos: #PF rip=0x{:X} cr2=0x{:X} err={:?}",
        frame.instruction_pointer.as_u64(),
        cr2.as_u64(),
        code
    );
    halt();
}

extern "x86-interrupt" fn df_handler(frame: InterruptStackFrame, _code: u64) -> ! {
    kprintln!("ruos: #DF rip=0x{:X}", frame.instruction_pointer.as_u64());
    halt();
}

extern "x86-interrupt" fn bp_handler(frame: InterruptStackFrame) {
    kprintln!("ruos: bp ok rip=0x{:X}", frame.instruction_pointer.as_u64());
    // Resumable — handler returns; CPU continues at rip past the int3 instruction.
}

fn halt() -> ! {
    loop {
        unsafe { core::arch::asm!("cli; hlt") };
    }
}
