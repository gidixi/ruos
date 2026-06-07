//! Kernel heap: global allocator (talc) and `init_heap()`.
//!
//! Backing memory comes from a region described by Limine's memory map,
//! accessed virtually via Limine's HHDM offset. The actual `MemmapRequest` /
//! `HhdmRequest` statics live in `main.rs` so they sit next to the other Limine
//! `.requests` items and inside the existing markers; this module reads them via
//! the `crate::` path.

use core::fmt;
use limine::memmap::MEMMAP_USABLE;
#[cfg(feature = "legacy-talc")]
use talc::{ErrOnOom, Span, Talc, Talck};

/// Heap size in bytes: 256 MiB. Large enough to hold MULTIPLE egui compositor
/// windows simultaneously: each egui wasm instance reserves ~48 MiB of guest
/// linear memory (its declared minimum) plus its ~9 MiB AOT module and software
/// raster buffers. SP-C's `wm.spawn` instantiates a SECOND egui window while the
/// first is live, so 128 MiB OOM'd the second linear-memory allocation
/// (`failed to allocate 0x3000000 bytes`). 256 MiB fits several windows; the QEMU
/// run-config gives 512 MiB RAM so the USABLE region is ample.
/// (Was 16 MiB — OOM'd the GUI; then 128 MiB — OOM'd the 2nd compositor window.)
pub const HEAP_SIZE: usize = 256 * 1024 * 1024;

// SMP baseline (migrazione shared-nothing, spec 2026-06-05): questo è un VERO
// spinlock SMP (spin 0.9.8, CAS cross-core), non uno stub single-core. È preso su
// OGNI alloc/free di OGNI core → è il collo di contesa #1 quando arriveranno gli
// executor per-core (Step 3). Il magazine per-core (alloc_magazine.rs) elimina
// questa contesa per le alloc piccole. NON è un problema di safety
// (audit CHANGELOG/186: 0 must-fix), è un problema di CONTESA.
#[cfg(feature = "legacy-talc")]
#[global_allocator]
pub static ALLOCATOR: Talck<spin::Mutex<()>, ErrOnOom> = Talc::new(ErrOnOom).lock();

#[cfg(not(feature = "legacy-talc"))]
#[global_allocator]
pub static ALLOCATOR: crate::memory::alloc_magazine::MagazineAlloc =
    crate::memory::alloc_magazine::MagazineAlloc::new();

/// The heap region claimed by `init_heap`, recorded so that the future physical
/// frame allocator (Step 6) can mask these frames as already-owned and never hand
/// them out again. Set exactly once on successful heap init.
static HEAP_INFO: spin::Once<HeapInfo> = spin::Once::new();

/// Returns the heap region (`None` before `init_heap` succeeds).
pub fn heap_region() -> Option<HeapInfo> {
    HEAP_INFO.get().copied()
}

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
            HeapInitError::NoMemoryMap    => f.write_str("no memory map"),
            HeapInitError::NoHhdm         => f.write_str("no hhdm"),
            HeapInitError::NoUsableRegion => f.write_str("no usable region"),
            HeapInitError::ClaimFailed    => f.write_str("claim failed"),
        }
    }
}

pub fn init_heap() -> Result<HeapInfo, HeapInitError> {
    let memmap = crate::MEMMAP_REQUEST.response().ok_or(HeapInitError::NoMemoryMap)?;
    let hhdm   = crate::HHDM_REQUEST.response().ok_or(HeapInitError::NoHhdm)?;
    let hhdm_offset = hhdm.offset;

    // The MEMMAP_USABLE filter is load-bearing for memory safety: it excludes the
    // kernel image, modules, bootloader-reclaimable regions, ACPI, MMIO, etc. Do
    // not broaden this predicate without revisiting the SAFETY argument below.
    let entry = memmap.entries()
        .iter()
        .find(|e| e.type_ == MEMMAP_USABLE && (e.length as usize) >= HEAP_SIZE)
        .ok_or(HeapInitError::NoUsableRegion)?;

    let phys_base = entry.base;
    let virt_base = phys_base + hhdm_offset;

    // SAFETY: `[virt_base, virt_base + HEAP_SIZE)` is the HHDM image of a Limine
    // USABLE memmap entry of at least HEAP_SIZE bytes. Limine maps it read/write
    // at `phys + hhdm_offset` for the lifetime of the kernel and guarantees it is
    // disjoint from the kernel image, the bootloader, and any other reclaimable
    // region. No other reference into this range exists at this point in boot, so
    // the talc allocator has exclusive ownership. `ALLOCATOR` is `'static`, so
    // the claimed span stays valid for as long as it is used.
    #[cfg(not(feature = "legacy-talc"))]
    unsafe { ALLOCATOR.claim(virt_base as *mut u8, HEAP_SIZE).map_err(|_| HeapInitError::ClaimFailed)?; }
    #[cfg(feature = "legacy-talc")]
    unsafe { ALLOCATOR.lock().claim(Span::from_base_size(virt_base as *mut u8, HEAP_SIZE)).map_err(|_| HeapInitError::ClaimFailed)?; }

    let info = HeapInfo { phys_base, virt_base, size: HEAP_SIZE };
    HEAP_INFO.call_once(|| info);
    Ok(info)
}
