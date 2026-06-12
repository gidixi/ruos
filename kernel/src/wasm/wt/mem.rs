//! The single audited path to a Wasmtime guest's linear memory (mirrors the
//! wasmi `wasm/host/mem.rs` rule: no raw guest reads/writes elsewhere).
//! Generic over the Store data type `T` — it only touches the `memory` export.
//!
//! Threaded modules (`wasm32-wasip1-threads`, MT Fase 2) re-export their
//! IMPORTED shared linear memory, so the export is an `Extern::SharedMemory`,
//! not an `Extern::Memory` — both variants are handled here. Shared accesses
//! are plain byte copies after a bounds check: concurrent guest writes to the
//! same buffer are the wasm shared-memory model (same as upstream
//! wasmtime-wasi), the guest owns that race.

use wasmtime::{Caller, Extern, Memory, SharedMemory};
use alloc::vec::Vec;

enum GuestMem {
    Plain(Memory),
    Shared(SharedMemory),
}

fn memory<T>(caller: &mut Caller<'_, T>) -> Option<GuestMem> {
    match caller.get_export("memory") {
        Some(Extern::Memory(m)) => Some(GuestMem::Plain(m)),
        Some(Extern::SharedMemory(s)) => Some(GuestMem::Shared(s)),
        _ => None,
    }
}

/// Copy `buf` into guest memory at `ptr`. Returns false if out of bounds.
pub fn write<T>(caller: &mut Caller<'_, T>, ptr: u32, buf: &[u8]) -> bool {
    match memory(caller) {
        Some(GuestMem::Plain(mem)) => mem.write(caller, ptr as usize, buf).is_ok(),
        Some(GuestMem::Shared(s)) => {
            let data = s.data();
            let start = ptr as usize;
            let end = match start.checked_add(buf.len()) {
                Some(e) => e,
                None => return false,
            };
            if end > data.len() {
                return false;
            }
            // SAFETY: bounds-checked above; data() stays valid for the whole
            // call (shared memory never moves). Concurrent guest access is
            // the shared-memory model — byte copies like upstream.
            unsafe {
                let dst = data.as_ptr().add(start) as *mut u8;
                core::ptr::copy_nonoverlapping(buf.as_ptr(), dst, buf.len());
            }
            true
        }
        None => false,
    }
}

/// Read `len` bytes from guest memory at `ptr`. None if out of bounds.
pub fn read<T>(caller: &mut Caller<'_, T>, ptr: u32, len: u32) -> Option<Vec<u8>> {
    match memory(caller)? {
        GuestMem::Plain(mem) => {
            let mut out = alloc::vec![0u8; len as usize];
            mem.read(caller, ptr as usize, &mut out).ok()?;
            Some(out)
        }
        GuestMem::Shared(s) => {
            let data = s.data();
            let start = ptr as usize;
            let end = start.checked_add(len as usize)?;
            if end > data.len() {
                return None;
            }
            let mut out = alloc::vec![0u8; len as usize];
            // SAFETY: bounds-checked above; see `write` for the race model.
            unsafe {
                let src = data.as_ptr().add(start) as *const u8;
                core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), len as usize);
            }
            Some(out)
        }
    }
}

/// Write a little-endian u32 to guest memory. False if out of bounds.
pub fn write_u32<T>(caller: &mut Caller<'_, T>, ptr: u32, val: u32) -> bool {
    write(caller, ptr, &val.to_le_bytes())
}
