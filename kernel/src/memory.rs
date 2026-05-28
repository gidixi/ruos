//! Kernel heap: global allocator (talc) and `init_heap()` (added in Task 2).
//!
//! Backing memory comes from a region described by Limine's memory map,
//! accessed virtually via Limine's HHDM offset.

use talc::{ErrOnOom, Talc, Talck};

/// Heap size in bytes: 4 MiB.
pub const HEAP_SIZE: usize = 4 * 1024 * 1024;

#[global_allocator]
pub static ALLOCATOR: Talck<spin::Mutex<()>, ErrOnOom> = Talc::new(ErrOnOom).lock();
