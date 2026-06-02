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
mod usb;
mod blockdev;
mod ahci;
mod rtc;
mod ssh;
mod sync;
mod cpu;
mod smp;
mod sched;
mod pipe;
mod service;

use core::panic::PanicInfo;
use limine::BaseRevision;
use limine::request::{FramebufferRequest, HhdmRequest, MemmapRequest, MpRequest, RsdpRequest};
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
#[link_section = ".requests"]
pub static MP_REQUEST: MpRequest = MpRequest::new(0);

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
    // Disable interrupts first — prevents re-entrant IRQ handlers from
    // trying to acquire locks we may already hold.
    x86_64::instructions::interrupts::disable();

    use core::fmt::Write as _;

    // Format the message once into a fixed stack buffer, then fan it out to
    // every sink via try_lock — never blocking.  If a lock is already held
    // (e.g. panic inside a serial write) we skip that sink rather than
    // deadlocking.  The old code used CONSOLE.lock() unconditionally, which
    // would deadlock whenever the panic occurred inside a logging call.
    let mut scratch = crate::klog::Scratch::new();
    let _ = writeln!(scratch, "\nKERNEL PANIC: {}", info);
    let msg = scratch.as_bytes();

    // klog ring — best-effort; no-op if ring lock is contested.
    crate::klog::try_push(msg);

    // Serial — try_lock; skip if the IRQ/printing path already holds it.
    // Note: CONSOLE also writes to serial via SERIAL.lock() internally, so
    // we only access one of the two paths here to avoid a double-lock.
    // We prefer raw SERIAL for reliability; the framebuffer path is attempted
    // only if the framebuffer console lock is free AND we can acquire SERIAL
    // a second time (it will be free because we dropped the guard above).
    if let Some(mut s) = crate::serial::SERIAL.try_lock() {
        let _ = s.write_str(core::str::from_utf8(msg).unwrap_or("KERNEL PANIC\n"));
    }

    // Framebuffer only — write directly to the FB sink, bypassing SerialConsole
    // (which calls SERIAL.lock() and would deadlock if serial is contested).
    if let Some(mut c) = crate::console::CONSOLE.try_lock() {
        if let Some(fb) = &mut c.fb {
            // FramebufferConsole has an inherent write_str; call it directly.
            crate::console::fb::FramebufferConsole::write_str(
                fb,
                core::str::from_utf8(msg).unwrap_or("KERNEL PANIC\n"),
            );
        }
    }

    // Feature-gated exit strategy:
    //   default (no `panic-halt`): controlled reset so the box recovers
    //     automatically instead of bricking until a power-cycle.
    //   `panic-halt` feature: halt-for-inspection (old behaviour) — useful
    //     when attached to a debugger or reading the serial log manually.
    #[cfg(feature = "panic-halt")]
    loop { x86_64::instructions::hlt(); }

    #[cfg(not(feature = "panic-halt"))]
    {
        crate::power::reboot();
        // reboot() is `-> !` and never returns; the loop below is unreachable
        // but satisfies the compiler's `-> !` type-check.
        #[allow(unreachable_code)]
        loop { x86_64::instructions::hlt(); }
    }
}

/// Halt and catch fire: disable interrupts and halt forever.
fn hcf() -> ! {
    loop {
        unsafe { core::arch::asm!("cli; hlt") };
    }
}
