//! Memory subsystem: heap (talc), physical frame allocator (bitmap), and
//! paging Mapper. Re-exports the most commonly used names so callers see a
//! single `crate::memory::*` API regardless of internal layout.

pub mod heap;
pub mod frames;
pub mod mapper;
pub mod tlb;
pub mod dma;
pub mod exec;

#[cfg(feature = "boot-checks")]
pub mod allocbench;

pub mod alloc_magazine;

pub use heap::{ALLOCATOR, HEAP_SIZE, HeapInfo, HeapInitError, init_heap, heap_region};
pub use frames::{FrameCounts, FrameInitError, allocate_frame, free_frame, frame_counts,
    init as init_frames};
// NB: `UnmapError` e `hhdm_offset` NON sono ri-esportati: tutti i call site li
// usano via `mapper::` diretto (re-export inutilizzato → warning).
pub use mapper::{MapError, init as init_mapper, map_page, unmap_page, map_io_page,
    map_io_range, hhdm_virt, set_flags, set_flags_range, unmap_range};
