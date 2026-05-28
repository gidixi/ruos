#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(impl_trait_in_assoc_type)]

extern crate alloc;

mod serial;
mod kprint;
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
mod executor;

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

    pic::disable();
    let acpi_info = match acpi_init::parse() {
        Ok(info) => info,
        Err(e) => {
            kprintln!("ruos: acpi fail: {}", e);
            hcf();
        }
    };
    kprintln!(
        "ruos: acpi ok lapic=0x{:X} ioapic=0x{:X} overrides={}",
        acpi_info.lapic_base, acpi_info.ioapic_base, acpi_info.overrides.len()
    );

    let frame_counts = match memory::init_frames() {
        Ok(c) => c,
        Err(e) => {
            kprintln!("ruos: frames fail: {}", e);
            hcf();
        }
    };
    kprintln!(
        "ruos: frames total={} used={} free={}",
        frame_counts.total, frame_counts.used, frame_counts.free,
    );

    memory::init_mapper(acpi_info.hhdm_offset);
    kprintln!("ruos: paging up");

    // Smoke test: map a fresh canonical lower-half VA, write/read, unmap.
    {
        use x86_64::structures::paging::PageTableFlags;
        let test_virt = x86_64::VirtAddr::new(0x4000_0000_0000);
        let frame = memory::allocate_frame().expect("smoke test: no frame");
        let phys = frame.start_address();
        let flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::NO_EXECUTE;
        if let Err(e) = memory::map_page(test_virt, phys, flags) {
            kprintln!("ruos: map test failed: {}", e);
            hcf();
        }
        unsafe { test_virt.as_mut_ptr::<u64>().write_volatile(0xC0FFEEu64); }
        let back = unsafe { test_virt.as_ptr::<u64>().read_volatile() };
        if back != 0xC0FFEE {
            kprintln!("ruos: map test mismatch: 0x{:X}", back);
            hcf();
        }
        memory::unmap_page(test_virt).expect("smoke test unmap");
        memory::free_frame(frame);
        kprintln!(
            "ruos: map test ok virt=0x{:X} phys=0x{:X}",
            test_virt.as_u64(),
            phys.as_u64(),
        );
    }

    apic::lapic::init(acpi_info.lapic_base, idt::VEC_SPURIOUS);
    apic::ioapic::init(acpi_info.ioapic_base);
    if let Err(e) = timer::init(100) {
        kprintln!("ruos: timer fail: {}", e);
        hcf();
    }

    keyboard::init(&acpi_info.overrides);

    x86_64::instructions::interrupts::enable(); // sti

    match vfs::init() {
        Ok(n) => kprintln!("ruos: vfs init ok mounts={}", n),
        Err(e) => {
            kprintln!("ruos: vfs init fail: {}", e);
            hcf();
        }
    }

    let smoke = vfs::block_on(async {
        use vfs::{open, write, read, seek, close, OpenFlags, Whence};
        // /dev/null write
        let fd = open("/dev/null", OpenFlags::WRITE).await?;
        write(fd, b"hello").await?;
        close(fd).await?;
        // /tmp/x: create, write, seek to start, read back.
        let fd = open(
            "/tmp/x",
            OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::READ,
        ).await?;
        write(fd, b"abc").await?;
        seek(fd, 0, Whence::Set).await?;
        let mut buf = [0u8; 8];
        let n = read(fd, &mut buf).await?;
        close(fd).await?;
        Ok::<(usize, [u8; 8]), vfs::VfsError>((n, buf))
    });
    match smoke {
        Ok((n, buf)) => kprintln!(
            "ruos: vfs smoke ok n={} buf=[{}]",
            n,
            core::str::from_utf8(&buf[..n]).unwrap_or("?"),
        ),
        Err(e) => {
            kprintln!("ruos: vfs smoke fail: {}", e);
            hcf();
        }
    }

    modules::mount_all();
    net::init();

    match console::fb_init::init() {
        Ok(mut fb) => {
            let (w, h, p, b) = fb.dims();
            kprintln!("ruos: fb ok {}x{} pitch={} bpp={}", w, h, p, b);
            let ok = console::fb::self_test(&mut fb);
            kprintln!("ruos: fb test {}", if ok { "ok" } else { "fail" });
            console::CONSOLE.lock().attach_framebuffer(fb);
            kprintln!("ruos: fb attached");
        }
        Err(e) => {
            kprintln!("ruos: fb fail: {}", e);
        }
    }

    kprintln!("\x1b[31mERR\x1b[0m hello via ansi");
    kprintln!("ruos: ansi test ok");

    executor::run();
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
