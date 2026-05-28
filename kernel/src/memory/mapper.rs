//! Paging Mapper: a single global `OffsetPageTable` driven by Limine's HHDM
//! offset, plus thin helpers used everywhere outside this module.

use core::fmt;
use spin::Mutex;
use x86_64::{PhysAddr, VirtAddr};
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{
    OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Mapper, Size4KiB,
};
use x86_64::structures::paging::mapper::{MapToError, UnmapError as XUnmapError};

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
        }
    }
}

pub fn init(hhdm_offset: u64) {
    HHDM_OFFSET.call_once(|| hhdm_offset);
    let (cr3_frame, _) = Cr3::read();
    let pml4_virt = cr3_frame.start_address().as_u64() + hhdm_offset;
    // SAFETY: `pml4_virt` is the HHDM image of the live PML4. We become the
    // sole writer to `MAPPER` for the lifetime of the kernel; the underlying
    // page tables are mutated only through the Mapper API.
    let pml4: &'static mut PageTable = unsafe { &mut *(pml4_virt as *mut PageTable) };
    let table = unsafe { OffsetPageTable::new(pml4, VirtAddr::new(hhdm_offset)) };
    *MAPPER.lock() = Some(table);
}

pub fn map_page(virt: VirtAddr, phys: PhysAddr, flags: PageTableFlags)
    -> Result<(), MapError>
{
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
    let (frame, flush) = mapper.unmap(page).map_err(|e| match e {
        XUnmapError::PageNotMapped => UnmapError::NotMapped,
        _ => UnmapError::NotMapped,
    })?;
    flush.flush();
    Ok(frame)
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
