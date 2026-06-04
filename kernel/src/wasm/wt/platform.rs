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
// TLS — a single pointer (single-threaded executor).
// ---------------------------------------------------------------------------

static TLS: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());

#[no_mangle]
pub extern "C" fn wasmtime_tls_get() -> *mut u8 {
    TLS.load(Ordering::SeqCst)
}

#[no_mangle]
pub extern "C" fn wasmtime_tls_set(ptr: *mut u8) {
    TLS.store(ptr, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Virtual memory — backed by the frame allocator + paging.
// ---------------------------------------------------------------------------

/// prot_flags bits (must match wasmtime capi: READ=1, WRITE=2, EXEC=4).
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
    let flags = prot_to_flags(prot_flags);
    for i in 0..pages {
        let frame = match allocate_frame() {
            Some(f) => f,
            None => return 1,
        };
        let va = VirtAddr::new(base + i * PAGE);
        if map_page(va, frame.start_address(), flags).is_err() {
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
