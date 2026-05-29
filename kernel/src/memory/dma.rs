//! DMA regions: physically-contiguous frames + their HHDM virtual alias.
//! Reused by virtio (rings/buffers) and AHCI (Step 15). Ring memory is normal
//! cacheable RAM (x86 is DMA-coherent) — NOT marked NO_CACHE.

use core::ptr::NonNull;
use x86_64::{PhysAddr, VirtAddr};
use x86_64::structures::paging::{PhysFrame, Size4KiB};
use virtio_drivers::{BufferDirection, Hal, PhysAddr as VPhysAddr};

use crate::memory::frames::{allocate_contiguous, free_contiguous, PAGE_SIZE};

#[derive(Debug, Copy, Clone)]
pub struct DmaRegion {
    pub phys:  PhysAddr,
    pub virt:  VirtAddr,
    pub pages: usize,
}

pub fn alloc(pages: usize) -> Option<DmaRegion> {
    let first = allocate_contiguous(pages as u64)?;
    let phys = first.start_address();
    let virt = crate::memory::mapper::hhdm_virt(phys);
    unsafe { core::ptr::write_bytes(virt.as_mut_ptr::<u8>(), 0, pages * PAGE_SIZE as usize); }
    Some(DmaRegion { phys, virt, pages })
}

pub fn dealloc(r: DmaRegion) {
    let first = PhysFrame::<Size4KiB>::containing_address(r.phys);
    free_contiguous(first, r.pages as u64);
}

/// virtio-drivers HAL for ruos: DMA via the contiguous frame allocator, MMIO via
/// map_io_range, identity share/unshare (no IOMMU on our x86 target).
pub struct KernelHal;

unsafe impl Hal for KernelHal {
    fn dma_alloc(pages: usize, _dir: BufferDirection) -> (VPhysAddr, NonNull<u8>) {
        let r = alloc(pages).expect("virtio: dma_alloc out of frames");
        (r.phys.as_u64(), NonNull::new(r.virt.as_mut_ptr::<u8>()).unwrap())
    }

    unsafe fn dma_dealloc(paddr: VPhysAddr, _vaddr: NonNull<u8>, pages: usize) -> i32 {
        let phys = PhysAddr::new(paddr);
        dealloc(DmaRegion { phys, virt: crate::memory::mapper::hhdm_virt(phys), pages });
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: VPhysAddr, size: usize) -> NonNull<u8> {
        let virt = crate::memory::mapper::map_io_range(PhysAddr::new(paddr), size)
            .expect("virtio: mmio map failed");
        NonNull::new(virt.as_mut_ptr::<u8>()).unwrap()
    }

    unsafe fn share(buffer: NonNull<[u8]>, _dir: BufferDirection) -> VPhysAddr {
        // SAFETY: all kernel RAM (DMA frames, heap, stack) is HHDM-mapped at
        // virt = phys + HHDM_OFFSET, so phys = virt - hhdm_offset(). virtio
        // only passes buffers it allocated via dma_alloc or kernel-owned
        // slices, all HHDM-resident, so the subtraction recovers a valid phys.
        let v = buffer.as_ptr() as *mut u8 as u64;
        v - crate::memory::mapper::hhdm_offset()
    }

    unsafe fn unshare(_paddr: VPhysAddr, _buffer: NonNull<[u8]>, _dir: BufferDirection) {
        // No bounce buffer / IOMMU: nothing to undo.
    }
}
