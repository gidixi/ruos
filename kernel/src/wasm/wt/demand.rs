//! Demand paging for the Wasmtime VA window.
//!
//! Wasmtime (no_std, `custom-virtual-memory`) reserves linear memory and code
//! memory through `wasmtime_mmap_new` / `wasmtime_mprotect` (see `platform.rs`).
//! Previously every reserved page committed a physical frame immediately — a
//! 48 MiB egui linear-memory minimum cost 48 MiB of frames even though the guest
//! touches a few MiB, OOM'ing the 3rd window. This module makes those ranges
//! *demand-paged*: a reserve only records a `WtRange`; the page-fault handler
//! (`idt::pf_handler`) commits a zeroed frame on first touch via `commit_fault`.
//!
//! See docs/superpowers/specs/2026-06-09-wt-linear-mem-demand-paging-design.md.
//!
//! Invariants:
//! * `RANGES` is a LEAF lock: taken only to read/update the registry, NEVER held
//!   across `map_page`/`set_flags` (which take MAPPER) or any WT-page access. So
//!   a #PF can never occur while this core holds `RANGES` (no reentrant deadlock).
//! * Only ranges whose prot includes R/W/X commit on fault. A fault in a
//!   PROT_NONE (reserved-only) page is a real bug — Wasmtime uses inline bounds
//!   checks (`signals_based_traps(false)`), so the guest never faults into a
//!   guard page; `commit_fault` returns false → the handler panics, as before.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::VirtAddr;
use x86_64::structures::paging::PageTableFlags;
use crate::sync::IrqMutex;
use crate::memory::{allocate_frame, free_frame, map_page, hhdm_virt, MapError};

const PAGE: u64 = 0x1000;

// Protection bits — MUST match platform.rs PROT_* (wasmtime capi: R=1,W=2,X=4).
const PROT_READ: u32  = 1 << 0;
const PROT_WRITE: u32 = 1 << 1;
const PROT_EXEC: u32  = 1 << 2;
const PROT_RWX: u32 = PROT_READ | PROT_WRITE | PROT_EXEC;

/// Base of the Wasmtime VA window. Distinct from `memory::exec`
/// (0xFFFF_E000_0000_0000) so the window can grow up to EXEC_BASE without
/// colliding. Reservations bump `NEXT` upward from here.
pub const WT_VM_BASE: u64 = 0xFFFF_D000_0000_0000;
static NEXT: AtomicU64 = AtomicU64::new(WT_VM_BASE);

#[derive(Clone, Copy)]
struct WtRange { base: u64, end: u64, prot: u32 }

static RANGES: IrqMutex<Vec<WtRange>> = IrqMutex::new(Vec::new());

/// Fast pre-filter: is `addr` inside the reserved part of the WT window? Lets the
/// #PF handler reject non-WT faults without locking `RANGES`.
#[inline]
pub fn in_window(addr: u64) -> bool {
    addr >= WT_VM_BASE && addr < NEXT.load(Ordering::SeqCst)
}

/// Reserve `pages` of fresh VA with the given prot, recording the range WITHOUT
/// committing any frame. Returns the base VA. (`pages == 0` still returns a base
/// so callers get a stable pointer.)
pub fn reserve(pages: u64, prot: u32) -> u64 {
    let bytes = pages * PAGE;
    let base = NEXT.fetch_add(bytes, Ordering::SeqCst);
    if bytes != 0 { set_prot(base, base + bytes, prot); }
    base
}

/// Set prot over `[base, end)`, splitting overlapping ranges so the registry
/// stays a partition. Outer pieces keep their old prot; the inner span gets
/// `prot`. Used by reserve (initial prot) and mprotect/remap (prot change).
pub fn set_prot(base: u64, end: u64, prot: u32) {
    if end <= base { return; }
    let mut g = RANGES.lock();
    let mut out: Vec<WtRange> = Vec::with_capacity(g.len() + 2);
    for r in g.iter() {
        if r.end <= base || r.base >= end {
            out.push(*r); // disjoint — keep
        } else {
            // Overlap: keep the outer slivers with their old prot, drop the inner.
            if r.base < base { out.push(WtRange { base: r.base, end: base, prot: r.prot }); }
            if r.end > end   { out.push(WtRange { base: end, end: r.end, prot: r.prot }); }
        }
    }
    out.push(WtRange { base, end, prot });
    *g = out;
}

/// Remove `[base, end)` from the registry (carving partial overlaps). Used by
/// munmap; the frames themselves are freed by the caller.
pub fn remove(base: u64, end: u64) {
    if end <= base { return; }
    let mut g = RANGES.lock();
    let mut out: Vec<WtRange> = Vec::with_capacity(g.len() + 1);
    for r in g.iter() {
        if r.end <= base || r.base >= end {
            out.push(*r);
        } else {
            if r.base < base { out.push(WtRange { base: r.base, end: base, prot: r.prot }); }
            if r.end > end   { out.push(WtRange { base: end, end: r.end, prot: r.prot }); }
        }
    }
    *g = out;
}

fn lookup(addr: u64) -> Option<u32> {
    let g = RANGES.lock();
    for r in g.iter() {
        if addr >= r.base && addr < r.end { return Some(r.prot); }
    }
    None
}

fn prot_to_flags(prot: u32) -> PageTableFlags {
    let mut f = PageTableFlags::PRESENT;
    if prot & PROT_WRITE != 0 { f |= PageTableFlags::WRITABLE; }
    if prot & PROT_EXEC == 0 { f |= PageTableFlags::NO_EXECUTE; }
    f
}

/// Commit a zeroed frame for a not-present fault at `cr2`, if it lands in a
/// committable WT range. Returns true iff handled (caller resumes execution).
///
/// `irqs_were_on` = IF of the faulting context. When set we re-enable IRQs while
/// allocating + mapping so this core still services TLB-shootdown IPIs: a peer
/// core blocked in `set_flags`/`unmap_page` (which broadcast a shootdown and wait
/// for every core's ack) would otherwise deadlock against us spinning on MAPPER.
/// `map_page` of a not-present page itself issues NO shootdown (x86 never caches
/// negative TLB entries), so committing is cheap and IPI-free.
pub fn commit_fault(cr2: u64, irqs_were_on: bool) -> bool {
    if !in_window(cr2) { return false; }
    let prot = match lookup(cr2) {
        Some(p) if p & PROT_RWX != 0 => p,
        _ => return false, // PROT_NONE (reserved guard) or unregistered → real fault
    };
    let page = cr2 & !(PAGE - 1);

    if irqs_were_on { x86_64::instructions::interrupts::enable(); }

    let frame = match allocate_frame() {
        Some(f) => f,
        None => return false, // genuine frame exhaustion → fall through to panic
    };
    let phys = frame.start_address();
    // Zero via the frame's HHDM alias (always kernel-writable) so we can map the
    // WT page directly with its FINAL flags (read-only / exec included) — no extra
    // writable mapping and no permission-downgrade shootdown.
    unsafe {
        core::ptr::write_bytes(hhdm_virt(phys).as_mut_ptr::<u8>(), 0, PAGE as usize);
    }
    match map_page(VirtAddr::new(page), phys, prot_to_flags(prot)) {
        Ok(()) => true,
        // A peer core committed the same page first (both took a not-present fault
        // on it): our frame is unused. Protection faults on an already-present page
        // never reach here — the handler only calls us for not-present faults.
        Err(MapError::AlreadyMapped) => { free_frame(frame); true }
        Err(_) => { free_frame(frame); false }
    }
}

/// Boot-check: a reserved-but-untouched range must consume ZERO frames (the whole
/// point of demand paging), and a touch must then commit exactly its page lazily.
#[cfg(feature = "boot-checks")]
pub fn self_test() -> bool {
    use crate::memory::frame_counts;
    const PAGES: u64 = 4096; // 16 MiB reserved, untouched

    let free_before = frame_counts().free;
    // Reserve 16 MiB RW — must commit NOTHING.
    let base = reserve(PAGES, PROT_READ | PROT_WRITE);
    if frame_counts().free != free_before {
        return false; // a reserve committed frames → not lazy
    }
    // Touch one page → exactly one data frame committed on fault (plus possibly a
    // few intermediate page-table frames), so free must DROP, not stay equal.
    let touched = base + 1234 * PAGE;
    // SAFETY: `touched` is in a committable RW WT range; the read faults into
    // `commit_fault`, which maps a zeroed frame, then the read returns 0.
    let v = unsafe { core::ptr::read_volatile(touched as *const u8) };
    if v != 0 { return false; } // zero-init contract broken
    if frame_counts().free >= free_before {
        return false; // touch did not commit a frame
    }
    // Clean up: free the one committed page and drop the range.
    let _ = crate::memory::unmap_page(VirtAddr::new(touched & !(PAGE - 1)))
        .map(crate::memory::free_frame);
    remove(base, base + PAGES * PAGE);
    true
}
