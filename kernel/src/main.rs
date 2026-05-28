#![no_std]
#![no_main]

extern crate alloc;

mod serial;
mod memory;

use core::fmt::Write;
use core::panic::PanicInfo;
use limine::BaseRevision;
use limine::{RequestsEndMarker, RequestsStartMarker};

/// Tell Limine which base revision we support.
#[used]
#[link_section = ".requests"]
static BASE_REVISION: BaseRevision = BaseRevision::new();

#[used]
#[link_section = ".requests_start_marker"]
static _START_MARKER: RequestsStartMarker = RequestsStartMarker::new();

#[used]
#[link_section = ".requests_end_marker"]
static _END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

#[no_mangle]
unsafe extern "C" fn kmain() -> ! {
    // Serial first: any failure below must be observable on the wire.
    let mut serial = serial::Serial::new();
    serial.init();
    let _ = serial.write_str("MinimalOS-rs: hello serial\n");

    if !BASE_REVISION.is_supported() {
        let _ = serial.write_str("MinimalOS-rs: unsupported Limine base revision\n");
    }

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
