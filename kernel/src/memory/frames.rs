//! Bitmap physical frame allocator.
//!
//! The bitmap is sized to cover the highest USABLE physical address the Limine
//! memory map mentions. Non-USABLE regions above that address (e.g. multi-GiB
//! high MMIO holes reported as RESERVED) are intentionally not tracked: we
//! could never hand those frames out anyway, and mirroring them would require
//! tens of MiB of bitmap on the modest kernel heap. Each bit is 1 = used,
//! 0 = free. The bitmap itself lives on the kernel heap.
//!
//! At init: every bit starts USED, then USABLE memmap entries are walked to
//! mark their frames FREE, then the heap region (already owned by talc) is
//! re-marked USED so we never hand the heap's backing frames back out.

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use limine::memmap::MEMMAP_USABLE;
use x86_64::PhysAddr;
use x86_64::structures::paging::{FrameAllocator, FrameDeallocator, PhysFrame, Size4KiB};

pub const PAGE_SIZE: u64 = 4096;

#[derive(Debug, Copy, Clone)]
pub struct FrameCounts {
    pub total: u64,
    pub used:  u64,
    pub free:  u64,
}

#[derive(Debug)]
pub enum FrameInitError {
    NoMemoryMap,
    NoUsableRegion,
}

impl fmt::Display for FrameInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameInitError::NoMemoryMap    => f.write_str("no memory map"),
            FrameInitError::NoUsableRegion => f.write_str("no usable region"),
        }
    }
}

pub struct Frames {
    bitmap: Vec<u64>,
    total:  u64,
    used:   u64,
}

impl Frames {
    fn new(total: u64) -> Self {
        // Tail bits past `total` in the last chunk stay set ("phantom used").
        // `allocate_frame`'s `frame >= self.total` guard guarantees we never
        // hand them out. Counter accounting stays honest by initializing
        // `used = total` (NOT `chunks * 64`); any future refactor must keep
        // these two invariants in lockstep.
        let chunks = ((total + 63) / 64) as usize;
        Frames { bitmap: vec![u64::MAX; chunks], total, used: total }
    }

    #[inline]
    fn idx(frame: u64) -> (usize, u32) {
        ((frame / 64) as usize, (frame % 64) as u32)
    }

    fn mark_used(&mut self, frame: u64) {
        if frame >= self.total { return; }
        let (i, b) = Self::idx(frame);
        if (self.bitmap[i] >> b) & 1 == 0 {
            self.bitmap[i] |= 1u64 << b;
            self.used += 1;
        }
    }

    fn mark_free(&mut self, frame: u64) {
        if frame >= self.total { return; }
        let (i, b) = Self::idx(frame);
        if (self.bitmap[i] >> b) & 1 == 1 {
            self.bitmap[i] &= !(1u64 << b);
            self.used -= 1;
        }
    }

    fn counts(&self) -> FrameCounts {
        FrameCounts { total: self.total, used: self.used, free: self.total - self.used }
    }

    /// Allocate `n` physically-contiguous free frames, returning the first
    /// frame. O(total) bitmap scan; marks all `n` used. None if no run fits.
    fn allocate_contiguous(&mut self, n: u64) -> Option<PhysFrame<Size4KiB>> {
        if n == 0 { return None; }
        let mut start: u64 = 0;
        let mut run: u64 = 0;
        let mut f: u64 = 0;
        while f < self.total {
            let (i, b) = Self::idx(f);
            let free = (self.bitmap[i] >> b) & 1 == 0;
            if free {
                if run == 0 { start = f; }
                run += 1;
                if run == n {
                    for g in start..start + n { self.bitmap[(g / 64) as usize] |= 1u64 << (g % 64); }
                    self.used += n;
                    return Some(PhysFrame::containing_address(PhysAddr::new(start * PAGE_SIZE)));
                }
            } else {
                run = 0;
            }
            f += 1;
        }
        None
    }

    fn free_contiguous(&mut self, first: PhysFrame<Size4KiB>, n: u64) {
        let base = first.start_address().as_u64() / PAGE_SIZE;
        for g in base..base + n { self.mark_free(g); }
    }
}

unsafe impl FrameAllocator<Size4KiB> for Frames {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        for (i, chunk) in self.bitmap.iter_mut().enumerate() {
            if *chunk != u64::MAX {
                let bit = chunk.trailing_ones();
                let frame = (i as u64) * 64 + (bit as u64);
                if frame >= self.total { return None; }
                *chunk |= 1u64 << bit;
                self.used += 1;
                return Some(PhysFrame::containing_address(PhysAddr::new(frame * PAGE_SIZE)));
            }
        }
        None
    }
}

impl FrameDeallocator<Size4KiB> for Frames {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        let f = frame.start_address().as_u64() / PAGE_SIZE;
        self.mark_free(f);
    }
}

// ORDINE LOCK (invariante SMP): chi serve sia MAPPER che FRAMES prende MAPPER PRIMA.
// FRAMES è preso da solo (allocate_frame/free_frame) o dopo MAPPER, mai prima.
pub(crate) static FRAMES: spin::Mutex<Option<Frames>> = spin::Mutex::new(None);

pub fn init() -> Result<FrameCounts, FrameInitError> {
    let memmap = crate::MEMMAP_REQUEST
        .response()
        .ok_or(FrameInitError::NoMemoryMap)?;

    // Size the bitmap to cover the highest USABLE address only. Non-USABLE
    // high regions (e.g. QEMU's multi-GiB high MMIO hole reported as type
    // RESERVED above 4 GiB) can be in the tens-of-GiB range and would blow
    // the kernel heap if we tried to mirror them in the bitmap. We can never
    // hand those frames out anyway, so we just truncate the address space we
    // describe.
    let mut max_phys: u64 = 0;
    let mut has_usable = false;
    for entry in memmap.entries().iter() {
        if entry.type_ != MEMMAP_USABLE { continue; }
        has_usable = true;
        let end = entry.base + entry.length;
        if end > max_phys { max_phys = end; }
    }
    if !has_usable { return Err(FrameInitError::NoUsableRegion); }

    let total_frames = (max_phys + PAGE_SIZE - 1) / PAGE_SIZE;
    let mut frames = Frames::new(total_frames);

    // Free every frame fully inside a USABLE entry.
    for entry in memmap.entries().iter() {
        if entry.type_ != MEMMAP_USABLE { continue; }
        let first = (entry.base + PAGE_SIZE - 1) / PAGE_SIZE;
        let last  = (entry.base + entry.length) / PAGE_SIZE;
        for f in first..last {
            frames.mark_free(f);
        }
    }

    // Heap frames are owned by talc — do not hand them back out.
    // Round first DOWN and last UP: any frame touched by the heap, even
    // partially, must be marked used. (This is the opposite rounding from
    // the USABLE walk above, which rounds inward to count only fully-usable
    // frames.)
    if let Some(info) = crate::memory::heap::heap_region() {
        let first = info.phys_base / PAGE_SIZE;
        let last  = (info.phys_base + info.size as u64 + PAGE_SIZE - 1) / PAGE_SIZE;
        for f in first..last {
            frames.mark_used(f);
        }
    }

    let counts = frames.counts();
    *FRAMES.lock() = Some(frames);
    Ok(counts)
}

pub fn allocate_frame() -> Option<PhysFrame<Size4KiB>> {
    FRAMES.lock().as_mut().and_then(|f| f.allocate_frame())
}

pub fn free_frame(frame: PhysFrame<Size4KiB>) {
    if let Some(f) = FRAMES.lock().as_mut() {
        unsafe { f.deallocate_frame(frame); }
    }
}

pub fn allocate_contiguous(n: u64) -> Option<PhysFrame<Size4KiB>> {
    FRAMES.lock().as_mut().and_then(|f| f.allocate_contiguous(n))
}

pub fn free_contiguous(first: PhysFrame<Size4KiB>, n: u64) {
    if let Some(f) = FRAMES.lock().as_mut() { f.free_contiguous(first, n); }
}

pub fn frame_counts() -> FrameCounts {
    FRAMES.lock()
        .as_ref()
        .map(|f| f.counts())
        .unwrap_or(FrameCounts { total: 0, used: 0, free: 0 })
}
