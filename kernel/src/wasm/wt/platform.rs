//! Platform-shim symbols required by no_std Wasmtime.
//!
//! Two groups:
//! * TLS get/set — Wasmtime needs one pointer of thread-local storage. ruos runs
//!   all wasm on the BSP (single-threaded cooperative executor) so one global
//!   pointer suffices.
//! * Virtual memory (`custom-virtual-memory` feature) — Wasmtime places AOT
//!   native code into executable pages via these mmap/mprotect-style calls. We
//!   back them with the kernel's frame allocator + paging. CoW memory images are
//!   declined (no_std, `memory_init_cow(false)`), and native signals are off, so
//!   no signal/trap symbols are needed.

use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use core::ffi::c_void;
use x86_64::VirtAddr;
use x86_64::structures::paging::PageTableFlags;
use crate::memory::{allocate_frame, free_frame, map_page, unmap_page, set_flags};

// ---------------------------------------------------------------------------
// TLS — one pointer PER CORE. Wasmtime stores per-activation CallThreadState
// here; with concurrent execution on multiple cores a single global pointer
// would be corrupted across cores, so index by cpu_id(). cpu_id() is the fast
// RDTSCP path (~23 ns) — cheap enough for the tls_get/set hot path.
// ---------------------------------------------------------------------------
use crate::cpu::MAX_CPUS;

static TLS: [AtomicPtr<u8>; MAX_CPUS] = {
    const Z: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
    [Z; MAX_CPUS]
};

#[no_mangle]
pub extern "C" fn wasmtime_tls_get() -> *mut u8 {
    TLS[crate::cpu::cpu_id() as usize].load(Ordering::SeqCst)
}

#[no_mangle]
pub extern "C" fn wasmtime_tls_set(ptr: *mut u8) {
    TLS[crate::cpu::cpu_id() as usize].store(ptr, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Custom sync primitives (`custom-sync-primitives` feature). no_std Wasmtime's
// default locks PANIC on contention; with this feature it calls these shims so
// multiple cores can run wasm concurrently. State lives inline in the 8-byte
// cell Wasmtime hands us (zero-init = unlocked). We spin with IRQs ENABLED so a
// waiting core still services timer + TLB-shootdown IPIs (no `cli` here).
// Locks are non-reentrant (matches std Mutex/RwLock semantics Wasmtime assumes).
// ---------------------------------------------------------------------------
use core::sync::atomic::AtomicUsize;

#[inline]
fn lock_cell(lock: *mut usize) -> &'static AtomicUsize {
    // SAFETY: Wasmtime guarantees `lock` points to a live, 8-byte-aligned cell
    // it zero-initialized and uses only via these shims for its lifetime.
    unsafe { &*(lock as *const AtomicUsize) }
}

#[no_mangle]
pub extern "C" fn wasmtime_sync_lock_acquire(lock: *mut usize) {
    let a = lock_cell(lock); // 0 = unlocked (zero-init), 1 = locked
    while a
        .compare_exchange_weak(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub extern "C" fn wasmtime_sync_lock_release(lock: *mut usize) {
    lock_cell(lock).store(0, Ordering::Release);
}

#[no_mangle]
pub extern "C" fn wasmtime_sync_lock_free(_lock: *mut usize) {
    // Inline state — nothing to free.
}

/// RwLock encoding in the cell: 0 = free, 1..=(MAX-1) = N readers,
/// usize::MAX = one writer (exclusive). Reader count never approaches MAX
/// (≤ MAX_CPUS concurrent readers), so the sentinel is unambiguous.
const RW_WRITER: usize = usize::MAX;

#[no_mangle]
pub extern "C" fn wasmtime_sync_rwlock_read(lock: *mut usize) {
    let a = lock_cell(lock);
    loop {
        let s = a.load(Ordering::Relaxed);
        if s != RW_WRITER
            && a.compare_exchange_weak(s, s + 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        {
            return;
        }
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub extern "C" fn wasmtime_sync_rwlock_read_release(lock: *mut usize) {
    lock_cell(lock).fetch_sub(1, Ordering::Release);
}

#[no_mangle]
pub extern "C" fn wasmtime_sync_rwlock_write(lock: *mut usize) {
    let a = lock_cell(lock);
    while a
        .compare_exchange_weak(0, RW_WRITER, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub extern "C" fn wasmtime_sync_rwlock_write_release(lock: *mut usize) {
    lock_cell(lock).store(0, Ordering::Release);
}

#[no_mangle]
pub extern "C" fn wasmtime_sync_rwlock_free(_lock: *mut usize) {
    // Inline state — nothing to free.
}

// ---------------------------------------------------------------------------
// Virtual memory — backed by the frame allocator + paging.
// ---------------------------------------------------------------------------

/// prot_flags bits (must match wasmtime capi: READ=1, WRITE=2, EXEC=4).
const PROT_READ: u32 = 1 << 0;
const PROT_WRITE: u32 = 1 << 1;
const PROT_EXEC: u32 = 1 << 2;
const PAGE: u64 = 0x1000;

/// Dedicated VA window for Wasmtime mappings (distinct from `memory::exec`).
const WT_VM_BASE: u64 = 0xFFFF_D000_0000_0000;
static NEXT: AtomicU64 = AtomicU64::new(WT_VM_BASE);

fn prot_to_flags(prot: u32) -> PageTableFlags {
    let mut f = PageTableFlags::PRESENT;
    if prot & PROT_WRITE != 0 {
        f |= PageTableFlags::WRITABLE;
    }
    if prot & PROT_EXEC == 0 {
        f |= PageTableFlags::NO_EXECUTE;
    }
    f
}

#[no_mangle]
pub extern "C" fn wasmtime_page_size() -> usize {
    PAGE as usize
}

/// Allocate `size` bytes of fresh virtual memory with the given protections.
/// Returns 0 on success and writes the base into `*ret`; non-zero on failure.
#[no_mangle]
pub extern "C" fn wasmtime_mmap_new(size: usize, prot_flags: u32, ret: *mut *mut u8) -> i32 {
    let pages = (size as u64 + PAGE - 1) / PAGE;
    let base = NEXT.fetch_add(pages * PAGE, Ordering::SeqCst);
    let final_flags = prot_to_flags(prot_flags);
    // wasm requires fresh memory to read as zero, and our frame allocator hands
    // back DIRTY frames. Wasmtime never memsets grown memory — it assumes the
    // platform mmap delivers zeroed pages (true on Linux/Windows). So we MUST
    // zero here. Crucially, wasm *linear memory* is created via `Mmap::reserve`,
    // i.e. `wasmtime_mmap_new(size, prot=0)` (PROT_NONE), and only later made
    // writable through `wasmtime_mprotect` (make_accessible). Zeroing only the
    // PROT_WRITE case therefore missed it entirely → stale frame bytes leaked
    // into the guest. egui/tiny-skia build the font atlas with `alloc_zeroed`,
    // which trusts the wasm zero-init guarantee and skips its own memset, so the
    // garbage surfaced as garbled glyphs (shapes write-before-read, so crisp).
    // Map each frame writable, zero it, then downgrade to the requested flags.
    let zero_flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
    let downgrade = (prot_flags & PROT_WRITE == 0) || (prot_flags & PROT_EXEC != 0);
    for i in 0..pages {
        let frame = match allocate_frame() {
            Some(f) => f,
            None => return 1,
        };
        let va = VirtAddr::new(base + i * PAGE);
        if map_page(va, frame.start_address(), zero_flags).is_err() {
            return 1;
        }
        // SAFETY: the page was just mapped writable for the whole 4 KiB.
        unsafe { core::ptr::write_bytes(va.as_mut_ptr::<u8>(), 0, PAGE as usize); }
        if downgrade && set_flags(va, final_flags).is_err() {
            return 1;
        }
    }
    // SAFETY: `ret` is a valid out-pointer supplied by Wasmtime.
    unsafe { *ret = base as *mut u8; }
    0
}

/// Replace the mapping covering `[addr, addr+size)` with a fresh, zeroed mapping
/// having the given protections (blank private mapping). prot 0 → unmap.
#[no_mangle]
pub extern "C" fn wasmtime_mmap_remap(addr: *mut u8, size: usize, prot_flags: u32) -> i32 {
    let base = addr as u64;
    let pages = (size as u64 + PAGE - 1) / PAGE;
    let flags = prot_to_flags(prot_flags);
    let writable = prot_flags & PROT_WRITE != 0;
    for i in 0..pages {
        let va = VirtAddr::new(base + i * PAGE);
        if let Ok(frame) = unmap_page(va) {
            free_frame(frame);
        }
        if prot_flags == 0 {
            continue; // erase: leave unmapped
        }
        let frame = match allocate_frame() {
            Some(f) => f,
            None => return 1,
        };
        // Map writable first so we can zero it, then downgrade to target prot.
        let tmp = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
        if map_page(va, frame.start_address(), tmp).is_err() {
            return 1;
        }
        // SAFETY: page is mapped writable for the whole 4 KiB.
        unsafe { core::ptr::write_bytes(va.as_mut_ptr::<u8>(), 0, PAGE as usize); }
        if !writable || (prot_flags & PROT_EXEC != 0) {
            if set_flags(va, flags).is_err() {
                return 1;
            }
        }
    }
    0
}

/// Unmap `[ptr, ptr+size)` and free the backing frames.
#[no_mangle]
pub extern "C" fn wasmtime_munmap(ptr: *mut u8, size: usize) -> i32 {
    let base = ptr as u64;
    let pages = (size as u64 + PAGE - 1) / PAGE;
    for i in 0..pages {
        let va = VirtAddr::new(base + i * PAGE);
        if let Ok(frame) = unmap_page(va) {
            free_frame(frame);
        }
    }
    0
}

/// Change protections on `[ptr, ptr+size)`.
#[no_mangle]
pub extern "C" fn wasmtime_mprotect(ptr: *mut u8, size: usize, prot_flags: u32) -> i32 {
    let base = ptr as u64;
    let pages = (size as u64 + PAGE - 1) / PAGE;
    let flags = prot_to_flags(prot_flags);
    for i in 0..pages {
        let va = VirtAddr::new(base + i * PAGE);
        if set_flags(va, flags).is_err() {
            return 1;
        }
    }
    0
}

// ---------------------------------------------------------------------------
// Memory images (CoW) — declined. Returning 0 with a NULL `*ret` tells Wasmtime
// "no image, fall back to regular memory" (see custom/vm.rs). map_at/free are
// then never invoked but must be defined for linking.
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn wasmtime_memory_image_new(
    _ptr: *const u8,
    _len: usize,
    ret: *mut *mut c_void,
) -> i32 {
    // SAFETY: `ret` is a valid out-pointer supplied by Wasmtime.
    unsafe { *ret = core::ptr::null_mut(); }
    0
}

#[no_mangle]
pub extern "C" fn wasmtime_memory_image_map_at(
    _image: *mut c_void,
    _addr: *mut u8,
    _len: usize,
) -> i32 {
    1 // never called (no image created); error if it ever is
}

#[no_mangle]
pub extern "C" fn wasmtime_memory_image_free(_image: *mut c_void) {}

/// Boot-check: verify the wasm zero-init contract across the reserve→commit
/// path that backs linear memory. Dirties a page, frees its frame, then
/// reserves a page with PROT_NONE (as `Mmap::reserve` does) and commits it
/// writable (as `make_accessible` does). Wasmtime never memsets grown memory,
/// so this committed page MUST read back as all-zero — the guarantee egui's
/// font atlas (`alloc_zeroed`) relies on. Returns true iff zeroed.
#[cfg(feature = "boot-checks")]
pub fn zero_init_self_test() -> bool {
    const N: usize = PAGE as usize;
    // 1) Commit a page writable, dirty it, free its frame back to the allocator
    //    so the reserve below is likely (LIFO) to reuse the dirtied frame.
    let mut p1: *mut u8 = core::ptr::null_mut();
    if wasmtime_mmap_new(N, PROT_READ | PROT_WRITE, &mut p1) != 0 {
        return false;
    }
    // SAFETY: [p1, p1+N) is mapped writable.
    unsafe { core::ptr::write_bytes(p1, 0xAB, N); }
    if wasmtime_munmap(p1, N) != 0 {
        return false;
    }
    // 2) Reserve (PROT_NONE) then commit writable — the exact linear-memory path.
    let mut p2: *mut u8 = core::ptr::null_mut();
    if wasmtime_mmap_new(N, 0, &mut p2) != 0 {
        return false;
    }
    if wasmtime_mprotect(p2, N, PROT_READ | PROT_WRITE) != 0 {
        return false;
    }
    // SAFETY: [p2, p2+N) is mapped writable.
    let zeroed = unsafe { core::slice::from_raw_parts(p2, N) }.iter().all(|&b| b == 0);
    let _ = wasmtime_munmap(p2, N);
    zeroed
}
