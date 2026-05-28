#![no_std]
#![no_main]

extern crate alloc;

mod serial;
mod memory;
mod gdt;

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
    use core::fmt::Write;
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    // Serial first: any failure below must be observable on the wire.
    let mut serial = serial::Serial::new();
    serial.init();
    let _ = serial.write_str("ruos: hello serial\n");

    if !BASE_REVISION.is_supported() {
        let _ = serial.write_str("ruos: unsupported Limine base revision\n");
        hcf();
    }

    // Heap init.
    let info = match memory::init_heap() {
        Ok(info) => info,
        Err(e) => {
            let _ = writeln!(serial, "ruos: heap fail: {}", e);
            hcf();
        }
    };
    let _ = writeln!(
        serial,
        "ruos: heap ok base=0x{:X} size={}",
        info.virt_base, info.size
    );

    // Smoke test: prove Box and Vec work through the global allocator.
    let b = Box::new(0xCAFEBABEu64);
    let v: Vec<u32> = (0..5).collect();
    let _ = writeln!(
        serial,
        "ruos: alloc box=0x{:X} vec={:?}",
        *b, v
    );

    gdt::init();

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
