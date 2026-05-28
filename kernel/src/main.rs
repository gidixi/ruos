#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod serial;
mod kprint;
mod memory;
mod gdt;
mod idt;

use core::panic::PanicInfo;
use limine::BaseRevision;
use limine::request::{HhdmRequest, MemmapRequest};
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
#[link_section = ".requests_start_marker"]
static _START_MARKER: RequestsStartMarker = RequestsStartMarker::new();

#[used]
#[link_section = ".requests_end_marker"]
static _END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

#[no_mangle]
unsafe extern "C" fn kmain() -> ! {
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    // Serial first: any failure below must be observable on the wire.
    crate::serial::SERIAL.lock().init();
    kprintln!("ruos: hello serial");

    if !BASE_REVISION.is_supported() {
        kprintln!("ruos: unsupported Limine base revision");
        hcf();
    }

    // Heap init.
    let info = match memory::init_heap() {
        Ok(info) => info,
        Err(e) => {
            kprintln!("ruos: heap fail: {}", e);
            hcf();
        }
    };
    kprintln!("ruos: heap ok base=0x{:X} size={}", info.virt_base, info.size);

    // Smoke test: prove Box and Vec work through the global allocator.
    let b = Box::new(0xCAFEBABEu64);
    let v: Vec<u32> = (0..5).collect();
    kprintln!("ruos: alloc box=0x{:X} vec={:?}", *b, v);

    // Step 5 — interrupt infrastructure.
    gdt::init();
    idt::init();
    kprintln!("ruos: idt up");

    // #BP smoke test: CPU traps are not maskable by IF, so `sti` is not required.
    core::arch::asm!("int3");

    hcf();
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    hcf();
}

/// Halt and catch fire: disable interrupts and halt forever.
fn hcf() -> ! {
    loop {
        unsafe { core::arch::asm!("cli; hlt") };
    }
}
