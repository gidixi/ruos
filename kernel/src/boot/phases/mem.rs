//! Phase 2 — memory: heap + ACPI parse + frame allocator + paging mapper.

use crate::boot::BootError;

pub fn init() -> Result<(), BootError> {
    // Heap.
    let info = crate::memory::init_heap()
        .map_err(|e| BootError::HeapInit(match e {
            crate::memory::HeapInitError::NoMemoryMap    => "no memory map",
            crate::memory::HeapInitError::NoHhdm         => "no hhdm",
            crate::memory::HeapInitError::NoUsableRegion => "no usable region",
            crate::memory::HeapInitError::ClaimFailed    => "claim failed",
        }))?;
    crate::binfo!("mem", "heap ok base=0x{:X} size={}", info.virt_base, info.size);

    #[cfg(feature = "boot-checks")]
    {
        use alloc::boxed::Box;
        use alloc::vec::Vec;
        let b = Box::new(0xCAFEBABEu64);
        let v: Vec<u32> = (0..5).collect();
        crate::binfo!("mem", "alloc smoke ok box=0x{:X} vec={:?}", *b, v);
    }

    // ACPI.
    let acpi_info = crate::acpi_init::parse()
        .map_err(|e| BootError::AcpiInit(match e {
            crate::acpi_init::AcpiInitError::NoRsdp        => "no rsdp",
            crate::acpi_init::AcpiInitError::NoHhdm        => "no hhdm",
            crate::acpi_init::AcpiInitError::RsdpBelowHhdm => "rsdp below hhdm",
            crate::acpi_init::AcpiInitError::Parse         => "parse",
            crate::acpi_init::AcpiInitError::NoLapic       => "no lapic",
            crate::acpi_init::AcpiInitError::NoIoapic      => "no ioapic",
        }))?;
    crate::binfo!(
        "mem",
        "acpi ok lapic=0x{:X} ioapic=0x{:X} overrides={}",
        acpi_info.lapic_base, acpi_info.ioapic_base, acpi_info.overrides.len()
    );

    // Frame allocator.
    let frame_counts = crate::memory::init_frames()
        .map_err(|e| BootError::FramesInit(match e {
            crate::memory::FrameInitError::NoMemoryMap    => "no memory map",
            crate::memory::FrameInitError::NoUsableRegion => "no usable region",
        }))?;
    crate::binfo!(
        "mem",
        "frames total={} used={} free={}",
        frame_counts.total, frame_counts.used, frame_counts.free
    );

    // Paging mapper.
    crate::memory::init_mapper(acpi_info.hhdm_offset);
    crate::binfo!("mem", "paging up");

    #[cfg(feature = "boot-checks")]
    {
        use x86_64::structures::paging::PageTableFlags;
        let test_virt = x86_64::VirtAddr::new(0x4000_0000_0000);
        let frame = crate::memory::allocate_frame()
            .ok_or(BootError::PagingInit("no frame for smoke test"))?;
        let phys = frame.start_address();
        let flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::NO_EXECUTE;
        crate::memory::map_page(test_virt, phys, flags)
            .map_err(|_| BootError::PagingInit("map failed"))?;
        unsafe { test_virt.as_mut_ptr::<u64>().write_volatile(0xC0FFEEu64) };
        let back = unsafe { test_virt.as_ptr::<u64>().read_volatile() };
        if back != 0xC0FFEE {
            return Err(BootError::PagingInit("paging read mismatch"));
        }
        crate::memory::unmap_page(test_virt)
            .map_err(|_| BootError::PagingInit("unmap failed"))?;
        crate::memory::free_frame(frame);
        crate::binfo!(
            "mem",
            "paging smoke ok virt=0x{:X} phys=0x{:X}",
            test_virt.as_u64(), phys.as_u64()
        );
    }

    // Store ACPI info for interrupts phase.
    super::set_acpi_info(acpi_info);

    Ok(())
}
