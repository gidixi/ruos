//! The single audited path to a Wasmtime guest's linear memory (mirrors the
//! wasmi `wasm/host/mem.rs` rule: no raw guest reads/writes elsewhere).
//! Generic over the Store data type `T` — it only touches the `memory` export.

use wasmtime::{Caller, Extern, Memory};
use alloc::vec::Vec;

fn memory<T>(caller: &mut Caller<'_, T>) -> Option<Memory> {
    match caller.get_export("memory") {
        Some(Extern::Memory(m)) => Some(m),
        _ => None,
    }
}

/// Copy `buf` into guest memory at `ptr`. Returns false if out of bounds.
pub fn write<T>(caller: &mut Caller<'_, T>, ptr: u32, buf: &[u8]) -> bool {
    match memory(caller) {
        Some(mem) => mem.write(caller, ptr as usize, buf).is_ok(),
        None => false,
    }
}

/// Read `len` bytes from guest memory at `ptr`. None if out of bounds.
pub fn read<T>(caller: &mut Caller<'_, T>, ptr: u32, len: u32) -> Option<Vec<u8>> {
    let mem = memory(caller)?;
    let mut out = alloc::vec![0u8; len as usize];
    mem.read(caller, ptr as usize, &mut out).ok()?;
    Some(out)
}

/// Write a little-endian u32 to guest memory. False if out of bounds.
pub fn write_u32<T>(caller: &mut Caller<'_, T>, ptr: u32, val: u32) -> bool {
    write(caller, ptr, &val.to_le_bytes())
}
