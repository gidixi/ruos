//! The single audited guest-memory boundary. EVERY host fn that touches guest
//! linear memory goes through here — no raw `mem.read`/`mem.write` elsewhere.
//!
//! One bug in a bound check here is total compromise (ring 0); one correct
//! check here makes every caller safe by construction. Never panics, never
//! indexes out of bounds; returns a WASI errno the caller propagates.

use wasmi::{AsContext, AsContextMut, Caller, Memory};
use alloc::vec::Vec;
use crate::wasm::state::RuntimeState;

/// Decimal WASI errnos used at the boundary.
pub const EINVAL: i32 = 28;
pub const EFAULT: i32 = 21;

/// Pure bound check — no wasmi types, host-testable. Returns Ok((off,len)) or errno.
pub(crate) fn check_bounds(ptr: i32, len: i32, size: u64) -> Result<(usize, usize), i32> {
    if ptr < 0 || len < 0 { return Err(EINVAL); }
    let end = (ptr as u64).checked_add(len as u64).ok_or(EFAULT)?;
    if end > size { return Err(EFAULT); }
    Ok((ptr as usize, len as usize))
}

/// Fetch the instance's exported linear memory, or EFAULT if absent.
fn memory(caller: &Caller<'_, RuntimeState>) -> Result<Memory, i32> {
    match caller.get_export("memory") {
        Some(wasmi::Extern::Memory(m)) => Ok(m),
        _ => Err(EFAULT),
    }
}

/// Read `len` bytes from guest memory at `ptr`. Bounds-checked. `len == 0` → empty.
pub fn guest_read(caller: &Caller<'_, RuntimeState>, ptr: i32, len: i32) -> Result<Vec<u8>, i32> {
    let mem = memory(caller)?;
    let size = mem.data_size(caller.as_context()) as u64;
    let (off, n) = check_bounds(ptr, len, size)?;
    let mut buf = alloc::vec![0u8; n];
    if n > 0 {
        mem.read(caller.as_context(), off, &mut buf).map_err(|_| EFAULT)?;
    }
    Ok(buf)
}

/// Read exactly `buf.len()` bytes from guest memory at `ptr` into `buf`.
pub fn guest_read_into(caller: &Caller<'_, RuntimeState>, ptr: i32, buf: &mut [u8]) -> Result<(), i32> {
    let mem = memory(caller)?;
    let size = mem.data_size(caller.as_context()) as u64;
    let len: i32 = buf.len().try_into().map_err(|_| EINVAL)?;
    let (off, _n) = check_bounds(ptr, len, size)?;
    if !buf.is_empty() {
        mem.read(caller.as_context(), off, buf).map_err(|_| EFAULT)?;
    }
    Ok(())
}

/// Write `bytes` into guest memory at `ptr`. Bounds-checked. Empty → no-op.
pub fn guest_write(caller: &mut Caller<'_, RuntimeState>, ptr: i32, bytes: &[u8]) -> Result<(), i32> {
    let mem = memory(caller)?;
    let size = mem.data_size(caller.as_context()) as u64;
    let len: i32 = bytes.len().try_into().map_err(|_| EINVAL)?;
    let (off, _n) = check_bounds(ptr, len, size)?;
    if !bytes.is_empty() {
        mem.write(caller.as_context_mut(), off, bytes).map_err(|_| EFAULT)?;
    }
    Ok(())
}

/// Write a little-endian u32 scalar at `ptr` (common for *_ptr out-params).
pub fn guest_write_u32(caller: &mut Caller<'_, RuntimeState>, ptr: i32, val: u32) -> Result<(), i32> {
    guest_write(caller, ptr, &val.to_le_bytes())
}
