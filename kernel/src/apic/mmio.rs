//! Tiny MMIO mapper used by LAPIC/IOAPIC bring-up.
//!
//! Limine's HHDM is only guaranteed to cover physical memory described by
//! the memory map (usable, bootloader-reclaimable, kernel, framebuffer).
//! LAPIC (0xFEE00000) and IOAPIC (0xFEC00000) MMIO pages sit outside that
//! range, so we walk the existing page tables (CR3 + HHDM offset) and add
//! a UC mapping for the requested page. New intermediate page table pages
//! are allocated from the kernel heap.
//!
//! This is single-shot bring-up code: one mapping per LAPIC/IOAPIC, then
//! never called again. Concurrency is irrelevant.

use alloc::boxed::Box;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::structures::paging::{PageTable, PageTableFlags};
use x86_64::registers::control::Cr3;

/// Map `phys` (4 KiB-aligned) to `phys + hhdm_offset` as UC (cache-disabled,
/// write-through). Idempotent: if the page is already mapped, this is a no-op.
pub fn map_mmio_page(phys: u64, hhdm_offset: u64) {
    assert!(phys & 0xFFF == 0, "mmio phys not 4 KiB aligned");
    let virt = phys + hhdm_offset;

    let (cr3_frame, _) = Cr3::read();
    let pml4_phys = cr3_frame.start_address().as_u64();
    let pml4 = unsafe { &mut *((pml4_phys + hhdm_offset) as *mut PageTable) };

    let pml4_i = ((virt >> 39) & 0x1FF) as usize;
    let pdpt_i = ((virt >> 30) & 0x1FF) as usize;
    let pd_i   = ((virt >> 21) & 0x1FF) as usize;
    let pt_i   = ((virt >> 12) & 0x1FF) as usize;

    let parent_flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
    // UC mapping: PCD=1, PWT=1, PRESENT|WRITABLE.
    let leaf_flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::WRITE_THROUGH
        | PageTableFlags::NO_CACHE;

    let pdpt = next_table_or_create(pml4, pml4_i, hhdm_offset, parent_flags);
    let pd   = next_table_or_create(pdpt, pdpt_i, hhdm_offset, parent_flags);
    let pt   = next_table_or_create(pd, pd_i, hhdm_offset, parent_flags);

    let entry = &mut pt[pt_i];
    if !entry.flags().contains(PageTableFlags::PRESENT) {
        entry.set_addr(
            x86_64::PhysAddr::new(phys),
            leaf_flags,
        );
    }

    // Invalidate TLB for the page so subsequent loads see the new mapping.
    x86_64::instructions::tlb::flush(x86_64::VirtAddr::new(virt));
}

/// Holding cell for page-table pages we allocate so the Box is never dropped.
/// Each PT is one heap allocation that must outlive the kernel.
static LEAKED: AtomicU64 = AtomicU64::new(0);

fn next_table_or_create<'a>(
    parent: &'a mut PageTable,
    index: usize,
    hhdm_offset: u64,
    parent_flags: PageTableFlags,
) -> &'a mut PageTable {
    let entry = &mut parent[index];
    if !entry.flags().contains(PageTableFlags::PRESENT) {
        // Allocate a zeroed PT from the heap. Box::leak it so the page stays
        // alive for the lifetime of the kernel; record the raw pointer so
        // the leak is intentional (and visible).
        let table = Box::new(PageTable::new());
        let raw = Box::into_raw(table);
        LEAKED.fetch_add(1, Ordering::Relaxed);
        let virt = raw as u64;
        // Heap VAs live inside the HHDM window — subtract to get phys.
        let phys = virt
            .checked_sub(hhdm_offset)
            .expect("pt page virt below HHDM offset");
        entry.set_addr(x86_64::PhysAddr::new(phys), parent_flags);
    }
    // SAFETY: entry is present; addr() points at a valid PT physical frame.
    let phys = entry.addr().as_u64();
    unsafe { &mut *((phys + hhdm_offset) as *mut PageTable) }
}
