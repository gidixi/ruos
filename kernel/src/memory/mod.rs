//! Memory subsystem: heap (talc), physical frame allocator (bitmap), and
//! paging Mapper. Re-exports the most commonly used names so callers see a
//! single `crate::memory::*` API regardless of internal layout.

pub mod heap;
pub mod frames;
pub mod mapper;
pub mod dma;

pub use heap::{ALLOCATOR, HEAP_SIZE, HeapInfo, HeapInitError, init_heap, heap_region};
pub use frames::{FrameCounts, FrameInitError, allocate_frame, free_frame, frame_counts,
    init as init_frames};
pub use mapper::{MapError, UnmapError, init as init_mapper, map_page, unmap_page, map_io_page,
    hhdm_virt};
