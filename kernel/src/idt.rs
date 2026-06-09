//! Interrupt Descriptor Table and CPU exception handlers.
//!
//! Hardware IRQ vectors (timer, keyboard) are declared as constants here and
//! the handlers themselves live in `timer.rs` / `keyboard.rs`.

use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use x86_64::VirtAddr;
use crate::{kprintln, gdt};

pub const VEC_LAPIC_TIMER: u8 = 0x20;
pub const VEC_KEYBOARD:    u8 = 0x21;
pub const VEC_MOUSE:       u8 = 0x22;
/// IPI vector the BSP sends to wake sleeping AP worker cores (SMP Fase 2).
pub const VEC_WAKE:        u8 = 0x40;
/// IPI vector: inter-core inbox delivery (Step 2). Handler marks this core's
/// inbox pending + EOIs; the core's run loop then drains its inbox.
pub const VEC_INBOX:         u8 = 0x41;
/// IPI vector for cross-core TLB shootdown (Step 3d). Handler: invlpg + ack + EOI.
pub const VEC_TLB_SHOOTDOWN: u8 = 0x42;
/// Reserved (Step 6 — supervisor core reset). No handler yet.
pub const VEC_RESET:         u8 = 0x43;
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

        idt[VEC_LAPIC_TIMER].set_handler_fn(crate::timer::timer_handler);
        idt[VEC_KEYBOARD].set_handler_fn(crate::keyboard::keyboard_handler);
        idt[VEC_MOUSE].set_handler_fn(crate::mouse::mouse_handler);
        idt[VEC_WAKE].set_handler_fn(wake_handler);
        idt[VEC_INBOX].set_handler_fn(inbox_handler);
        idt[VEC_TLB_SHOOTDOWN].set_handler_fn(tlb_shootdown_handler);

        idt
    });
    idt.load();
}

/// Load the already-built IDT on the current core (used by APs). `init()` must
/// have run on the BSP first to build the shared IDT.
pub fn load() {
    IDT.get().expect("idt::init() not called before idt::load()").load();
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
    // Not-present fault inside the Wasmtime VA window → demand-commit a zeroed
    // frame and resume (lazy linear-memory / code paging, see wt::demand). A
    // PROTECTION_VIOLATION (present page, wrong perms) is never our lazy commit —
    // it falls through to the panic path. We pass the faulting context's IF so
    // commit_fault can re-enable IRQs while spinning on MAPPER (TLB-shootdown
    // deadlock avoidance) only when it was safe to (the guest runs with IF=1).
    if !code.contains(PageFaultErrorCode::PROTECTION_VIOLATION) {
        let irqs_on = frame
            .cpu_flags
            .contains(x86_64::registers::rflags::RFlags::INTERRUPT_FLAG);
        if crate::wasm::wt::demand::commit_fault(cr2.as_u64(), irqs_on) {
            return;
        }
    }
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

/// AP wake IPI handler. No-op beyond EOI — its only purpose is to pull a
/// sleeping AP out of `hlt` so its worker loop re-checks the job queue.
extern "x86-interrupt" fn wake_handler(_frame: InterruptStackFrame) {
    crate::apic::lapic::eoi();
}

/// Inbox-delivery IPI handler. Marks this core's inbox as pending so its loop
/// (executor poll / AP worker) drains it, then EOIs.
extern "x86-interrupt" fn inbox_handler(_frame: InterruptStackFrame) {
    crate::smp::inbox::mark_pending(crate::cpu::cpu_id());
    crate::apic::lapic::eoi();
}

/// TLB shootdown IPI handler (Step 3d). Invalidates the page the mutator core
/// stored in SHOOT_ADDR, increments the ack counter so the mutator can proceed,
/// then EOIs. Must run on every non-mutator core before the mutator releases MAPPER.
extern "x86-interrupt" fn tlb_shootdown_handler(_frame: InterruptStackFrame) {
    crate::memory::tlb::on_ipi();
    crate::apic::lapic::eoi();
}

fn halt() -> ! {
    loop {
        unsafe { core::arch::asm!("cli; hlt") };
    }
}
