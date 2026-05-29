#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(impl_trait_in_assoc_type)]

extern crate alloc;

mod serial;
mod kprint;
mod klog;
mod proc;
mod power;
mod memory;
mod gdt;
mod idt;
mod pic;
mod acpi_init;
mod apic;
mod timer;
mod keyboard;
mod vfs;
mod modules;
mod console;
mod wasm;
mod net;
mod rng;
mod executor;
mod boot;
mod pty;
mod pci;
mod blockdev;

use core::panic::PanicInfo;
use limine::BaseRevision;
use limine::request::{FramebufferRequest, HhdmRequest, MemmapRequest, RsdpRequest};
use limine::{RequestsEndMarker, RequestsStartMarker};

/// Tell Limine which base revision we support.
#[used]
#[link_section = ".requests"]
static BASE_REVISION: BaseRevision = BaseRevision::new();

#[used]
#[link_section = ".requests"]
pub static MEMMAP_REQUEST: MemmapRequest = MemmapRequest::new();

#[used]
#[link_section = ".requests"]
pub static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

#[used]
#[link_section = ".requests"]
pub static RSDP_REQUEST: RsdpRequest = RsdpRequest::new();

#[used]
#[link_section = ".requests"]
pub static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

#[used]
#[link_section = ".requests_start_marker"]
static _START_MARKER: RequestsStartMarker = RequestsStartMarker::new();

#[used]
#[link_section = ".requests_end_marker"]
static _END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

#[no_mangle]
unsafe extern "C" fn kmain() -> ! {
    // Serial first: any failure below must be observable on the wire.
    crate::serial::SERIAL.lock().init();

    if !BASE_REVISION.is_supported() {
        kprintln!("ruos: limine base revision not supported");
        hcf();
    }

    // Calibrate TSC against PIT — gives sub-millisecond boot clock for
    // log lines that fire before the LAPIC timer comes up in `interrupts::init`.
    boot::clock::init();

    boot::banner::stamp();

    match boot::run() {
        Ok(_never) => unreachable!(),
        Err(e) => {
            crate::berr!("boot", "phase failed: {:?}", e);
            hcf();
        }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Disable interrupts first to avoid deadlock on CONSOLE lock.
    x86_64::instructions::interrupts::disable();
    // Try to print panic info (best-effort; may not work if CONSOLE is locked).
    use core::fmt::Write as _;
    let _ = writeln!(crate::console::CONSOLE.lock(), "KERNEL PANIC: {}", info);
    hcf();
}

/// Halt and catch fire: disable interrupts and halt forever.
fn hcf() -> ! {
    loop {
        unsafe { core::arch::asm!("cli; hlt") };
    }
}
