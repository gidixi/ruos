//! Paging Mapper: a single global `OffsetPageTable` driven by Limine's HHDM
//! offset, plus thin helpers used everywhere outside this module.

use core::fmt;
use spin::Mutex;
use x86_64::{PhysAddr, VirtAddr};
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{
    OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Mapper, Size4KiB,
};
use x86_64::structures::paging::mapper::{MapToError, UnmapError as XUnmapError, FlagUpdateError};

// ORDINE LOCK (invariante SMP): MAPPER.lock() PRIMA di FRAMES.lock(), mai invertito.
// map_page acquisisce MAPPER poi (via il frame allocator) FRAMES. Non tenere nessuno
// dei due attraverso un await o un send/wait di messaggio cross-core.
static MAPPER: Mutex<Option<OffsetPageTable<'static>>> = Mutex::new(None);
static HHDM_OFFSET: spin::Once<u64> = spin::Once::new();

#[derive(Debug)]
pub enum MapError {
    NotInitialized,
    AlreadyMapped,
    NoFrame,
    ParentHugePage,
}

#[derive(Debug)]
pub enum UnmapError {
    NotInitialized,
    NotMapped,
    ParentHugePage,
    InvalidFrame,
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MapError::NotInitialized => f.write_str("not initialized"),
            MapError::AlreadyMapped  => f.write_str("already mapped"),
            MapError::NoFrame        => f.write_str("no frame"),
            MapError::ParentHugePage => f.write_str("parent huge-page"),
        }
    }
}

impl fmt::Display for UnmapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnmapError::NotInitialized => f.write_str("not initialized"),
            UnmapError::NotMapped      => f.write_str("not mapped"),
            UnmapError::ParentHugePage => f.write_str("parent huge-page"),
            UnmapError::InvalidFrame   => f.write_str("invalid frame"),
        }
    }
}

pub fn init(hhdm_offset: u64) {
    if HHDM_OFFSET.get().is_some() { return; } // idempotent: avoid split-brain
    HHDM_OFFSET.call_once(|| hhdm_offset);
    let (cr3_frame, _) = Cr3::read();
    let pml4_virt = cr3_frame.start_address().as_u64() + hhdm_offset;
    // SAFETY: invariant for the `&'static mut PageTable` below: this Mapper
    // is the sole `&mut` walker of PML4 storage in the entire kernel. Limine
    // hands off after boot and writes nothing further; no other module
    // fabricates a `&mut PageTable` from the HHDM image of PML4 (the prior
    // ad-hoc walker in `apic/mmio.rs` is gone). All PML4 mutations flow
    // through `MAPPER` and are serialized by its `spin::Mutex`.
    let pml4: &'static mut PageTable = unsafe { &mut *(pml4_virt as *mut PageTable) };
    let table = unsafe { OffsetPageTable::new(pml4, VirtAddr::new(hhdm_offset)) };
    *MAPPER.lock() = Some(table);
}

pub fn map_page(virt: VirtAddr, phys: PhysAddr, flags: PageTableFlags)
    -> Result<(), MapError>
{
    // LOCK ORDER: MAPPER then FRAMES, never the reverse. Any future caller
    // that holds FRAMES and then enters map_page would deadlock; keep new
    // call sites consistent with this order.
    let mut g_map = MAPPER.lock();
    let mapper = g_map.as_mut().ok_or(MapError::NotInitialized)?;
    let mut g_frames = crate::memory::frames::FRAMES.lock();
    let frames = g_frames.as_mut().ok_or(MapError::NoFrame)?;

    let page: Page<Size4KiB> = Page::containing_address(virt);
    let frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(phys);

    // SAFETY: caller is responsible for the semantic safety of the mapping
    // (no aliasing of kernel-only memory into a hostile owner). The typed
    // Mapper itself rejects structural errors (huge-page parent, already-mapped).
    unsafe {
        mapper.map_to(page, frame, flags, frames)
            .map_err(|e| match e {
                MapToError::FrameAllocationFailed => MapError::NoFrame,
                MapToError::PageAlreadyMapped(_)  => MapError::AlreadyMapped,
                MapToError::ParentEntryHugePage   => MapError::ParentHugePage,
            })?
            .flush();
    }
    Ok(())
}

pub fn unmap_page(virt: VirtAddr) -> Result<PhysFrame<Size4KiB>, UnmapError> {
    let mut g_map = MAPPER.lock();
    let mapper = g_map.as_mut().ok_or(UnmapError::NotInitialized)?;
    let page: Page<Size4KiB> = Page::containing_address(virt);
    // Exhaustive match: future x86_64-crate UnmapError variants break compile
    // rather than getting silently folded into "not mapped".
    let (frame, flush) = mapper.unmap(page).map_err(|e| match e {
        XUnmapError::PageNotMapped         => UnmapError::NotMapped,
        XUnmapError::ParentEntryHugePage   => UnmapError::ParentHugePage,
        XUnmapError::InvalidFrameAddress(_) => UnmapError::InvalidFrame,
    })?;
    flush.flush();
    Ok(frame)
}

/// Change the flags of an already-mapped 4 KiB page (and flush its TLB entry).
/// Used to flip executable-memory pages from W (writable, NX) to X (read-only,
/// executable) — the W^X protection step.
pub fn set_flags(virt: VirtAddr, flags: PageTableFlags) -> Result<(), UnmapError> {
    let mut g_map = MAPPER.lock();
    let mapper = g_map.as_mut().ok_or(UnmapError::NotInitialized)?;
    let page: Page<Size4KiB> = Page::containing_address(virt);
    // SAFETY: caller guarantees `flags` is a valid combination for an existing
    // mapping; update_flags does not change the frame, only its permissions.
    unsafe {
        mapper.update_flags(page, flags)
            .map_err(|e| match e {
                FlagUpdateError::PageNotMapped       => UnmapError::NotMapped,
                FlagUpdateError::ParentEntryHugePage => UnmapError::ParentHugePage,
            })?
            .flush();
    }
    Ok(())
}

/// Virtual (HHDM) alias of a physical address. Valid for any RAM/MMIO phys
/// because Limine's HHDM covers all physical memory.
pub fn hhdm_virt(phys: PhysAddr) -> VirtAddr {
    let hhdm = *HHDM_OFFSET.get().expect("mapper: hhdm not initialized");
    VirtAddr::new(phys.as_u64() + hhdm)
}

/// The HHDM offset (phys→virt delta). Panics if paging not initialized.
pub fn hhdm_offset() -> u64 {
    *HHDM_OFFSET.get().expect("mapper: hhdm not initialized")
}

pub fn map_io_page(phys: PhysAddr) -> Result<VirtAddr, MapError> {
    let hhdm = *HHDM_OFFSET.get().ok_or(MapError::NotInitialized)?;
    let virt = VirtAddr::new(phys.as_u64() + hhdm);
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::WRITE_THROUGH
        | PageTableFlags::NO_CACHE
        | PageTableFlags::NO_EXECUTE;
    match map_page(virt, phys, flags) {
        Ok(()) => Ok(virt),
        Err(MapError::AlreadyMapped) => Ok(virt),
        Err(e) => Err(e),
    }
}

/// Map a multi-page MMIO window: every 4 KiB page covering `[phys, phys+bytes)`
/// is mapped (uncached) via `map_io_page`. Returns the virt of `phys` itself.
/// Idempotent per page.
pub fn map_io_range(phys: PhysAddr, bytes: usize) -> Result<VirtAddr, MapError> {
    let start = phys.as_u64() & !0xFFF;
    let end = (phys.as_u64() + bytes as u64 + 0xFFF) & !0xFFF;
    let mut p = start;
    while p < end {
        map_io_page(PhysAddr::new(p))?;
        p += 0x1000;
    }
    Ok(hhdm_virt(phys))
}
