//! Kernel heap: global allocator (talc) and `init_heap()` (added in Task 2).
//!
//! Backing memory comes from a region described by Limine's memory map,
//! accessed virtually via Limine's HHDM offset.

use talc::{ErrOnOom, Talc, Talck};

/// Heap size in bytes: 4 MiB.
pub const HEAP_SIZE: usize = 4 * 1024 * 1024;

#[global_allocator]
pub static ALLOCATOR: Talck<spin::Mutex<()>, ErrOnOom> = Talc::new(ErrOnOom).lock();

use core::fmt;
use limine::memmap::MEMMAP_USABLE;
use talc::Span;

/// The actual `MemmapRequest` / `HhdmRequest` statics live in `main.rs` so they
/// sit next to the other Limine `.requests` items and inside the existing markers.
/// This module reads them via the `crate::` path.

#[derive(Debug, Copy, Clone)]
pub struct HeapInfo {
    pub phys_base: u64,
    pub virt_base: u64,
    pub size: usize,
}

#[derive(Debug, Copy, Clone)]
pub enum HeapInitError {
    NoMemoryMap,
    NoHhdm,
    NoUsableRegion,
    ClaimFailed,
}

impl fmt::Display for HeapInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HeapInitError::NoMemoryMap     => f.write_str("no memory map"),
            HeapInitError::NoHhdm          => f.write_str("no hhdm"),
            HeapInitError::NoUsableRegion  => f.write_str("no usable region"),
            HeapInitError::ClaimFailed     => f.write_str("claim"),
        }
    }
}

pub fn init_heap() -> Result<HeapInfo, HeapInitError> {
    let memmap = crate::MEMMAP_REQUEST.response().ok_or(HeapInitError::NoMemoryMap)?;
    let hhdm   = crate::HHDM_REQUEST.response().ok_or(HeapInitError::NoHhdm)?;
    let hhdm_offset = hhdm.offset;

    let entry = memmap.entries()
        .iter()
        .find(|e| e.type_ == MEMMAP_USABLE && (e.length as usize) >= HEAP_SIZE)
        .ok_or(HeapInitError::NoUsableRegion)?;

    let phys_base = entry.base;
    let virt_base = phys_base + hhdm_offset;

    unsafe {
        ALLOCATOR
            .lock()
            .claim(Span::from_base_size(virt_base as *mut u8, HEAP_SIZE))
            .map_err(|_| HeapInitError::ClaimFailed)?;
    }

    Ok(HeapInfo { phys_base, virt_base, size: HEAP_SIZE })
}
