//! Executable-memory allocator (W^X) for AOT/JIT native code.
//!
//! A dedicated higher-half virtual window is bump-allocated per page. Frames are
//! aliased ONLY in this window (never given exec rights via the HHDM), so a page
//! is writable XOR executable, never both. Lifecycle: `alloc_exec` (writable,
//! NX) → write code → `protect_exec` (read-only, executable) → call → `free_exec`.

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::VirtAddr;
use x86_64::structures::paging::PageTableFlags;
use crate::memory::{allocate_frame, free_frame, map_page, unmap_page, set_flags};

/// Base of the executable virtual window. Higher-half, canonical, and outside
/// the HHDM image and kernel sections. 4 KiB-granular bump allocation upward.
const EXEC_BASE: u64 = 0xFFFF_E000_0000_0000;
static NEXT: AtomicU64 = AtomicU64::new(EXEC_BASE);

/// A live executable allocation. `ptr` is the start of `pages * 4 KiB`.
pub struct ExecAlloc {
    pub ptr: *mut u8,
    pub len: usize,
    pages: usize,
}

#[derive(Debug)]
pub enum ExecError { NoFrame, Map, Protect }

/// Reserve `len` bytes (rounded up to whole pages) as writable, non-executable
/// memory for code emission. Write into `ptr`, then call `protect_exec`.
pub fn alloc_exec(len: usize) -> Result<ExecAlloc, ExecError> {
    let pages = (len + 0xFFF) / 0x1000;
    let base = NEXT.fetch_add((pages as u64) * 0x1000, Ordering::SeqCst);
    let wflags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_EXECUTE;
    for i in 0..pages {
        let frame = allocate_frame().ok_or(ExecError::NoFrame)?;
        let virt = VirtAddr::new(base + (i as u64) * 0x1000);
        map_page(virt, frame.start_address(), wflags).map_err(|_| ExecError::Map)?;
    }
    Ok(ExecAlloc { ptr: base as *mut u8, len, pages })
}

/// Flip the allocation to read-only + executable (W^X protect step).
pub fn protect_exec(a: &ExecAlloc) -> Result<(), ExecError> {
    // PRESENT only: not WRITABLE, NO_EXECUTE cleared → read + execute.
    let rxflags = PageTableFlags::PRESENT;
    let base = a.ptr as u64;
    for i in 0..a.pages {
        let virt = VirtAddr::new(base + (i as u64) * 0x1000);
        set_flags(virt, rxflags).map_err(|_| ExecError::Protect)?;
    }
    Ok(())
}

/// Unmap and free every frame of the allocation.
pub fn free_exec(a: ExecAlloc) {
    let base = a.ptr as u64;
    for i in 0..a.pages {
        let virt = VirtAddr::new(base + (i as u64) * 0x1000);
        if let Ok(frame) = unmap_page(virt) {
            free_frame(frame);
        }
    }
}

/// Boot-check self-test: emit `mov eax,42; ret`, protect, call, expect 42.
#[cfg(feature = "boot-checks")]
pub fn self_test() -> bool {
    // x86-64: B8 2A 00 00 00 = mov eax,42 ; C3 = ret. (eax zero-extends into rax)
    let code: [u8; 6] = [0xB8, 0x2A, 0x00, 0x00, 0x00, 0xC3];
    let a = match alloc_exec(code.len()) {
        Ok(a) => a,
        Err(_) => return false,
    };
    // SAFETY: `a.ptr` covers `code.len()` writable bytes just mapped.
    unsafe { core::ptr::copy_nonoverlapping(code.as_ptr(), a.ptr, code.len()); }
    if protect_exec(&a).is_err() {
        return false;
    }
    // SAFETY: the bytes form a valid extern "C" fn() -> u64; pages are now RX.
    let f: extern "C" fn() -> u64 = unsafe { core::mem::transmute(a.ptr) };
    let r = f();
    free_exec(a);
    r == 42
}
