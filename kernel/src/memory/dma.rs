//! DMA regions: physically-contiguous frames + their HHDM virtual alias.
//! Reused by virtio (rings/buffers) and AHCI (Step 15). Ring memory is normal
//! cacheable RAM (x86 is DMA-coherent) — NOT marked NO_CACHE.

use x86_64::{PhysAddr, VirtAddr};
use x86_64::structures::paging::{PhysFrame, Size4KiB};

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
