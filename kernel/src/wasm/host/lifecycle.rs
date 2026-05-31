//! WASIX lifecycle host fns: args, environ, proc_exit.

use wasmi::{Caller, Error, Linker, Memory};
use crate::wasm::state::RuntimeState;

pub fn args_sizes_get(
    mut caller: Caller<'_, RuntimeState>,
    argc_ptr: i32,
    argv_buf_size_ptr: i32,
) -> Result<i32, Error> {
    let argc = caller.data().args.len() as u32;
    let argv_buf: u32 = caller.data().args.iter().map(|a| a.len() as u32 + 1).sum();
    if let Err(e) = crate::wasm::host::mem::guest_write_u32(&mut caller, argc_ptr, argc) {
        return Ok(e);
    }
    if let Err(e) = crate::wasm::host::mem::guest_write_u32(&mut caller, argv_buf_size_ptr, argv_buf) {
        return Ok(e);
    }
    Ok(0)
}

pub fn args_get(
    mut caller: Caller<'_, RuntimeState>,
    argv_ptr: i32,
    argv_buf_ptr: i32,
) -> Result<i32, Error> {
    let args = caller.data().args.clone();
    let mut cursor = argv_buf_ptr;
    for (i, arg) in args.iter().enumerate() {
        let slot_ptr = argv_ptr.wrapping_add((i * 4) as i32);
        if let Err(e) = crate::wasm::host::mem::guest_write_u32(&mut caller, slot_ptr, cursor as u32) {
            return Ok(e);
        }
        let mut owned = arg.clone();
        owned.push(0u8); // null terminator
        if let Err(e) = crate::wasm::host::mem::guest_write(&mut caller, cursor, &owned) {
            return Ok(e);
        }
        cursor = cursor.wrapping_add(owned.len() as i32);
    }
    Ok(0)
}

pub fn environ_sizes_get(
    mut caller: Caller<'_, RuntimeState>,
    environc_ptr: i32,
    environ_buf_size_ptr: i32,
) -> Result<i32, Error> {
    if let Err(e) = crate::wasm::host::mem::guest_write_u32(&mut caller, environc_ptr, 0) {
        return Ok(e);
    }
    if let Err(e) = crate::wasm::host::mem::guest_write_u32(&mut caller, environ_buf_size_ptr, 0) {
        return Ok(e);
    }
    Ok(0)
}

pub fn environ_get(
    _caller: Caller<'_, RuntimeState>,
    _environ_ptr: i32,
    _environ_buf_ptr: i32,
) -> Result<i32, Error> {
    Ok(0)
}

pub fn proc_exit(
    caller: Caller<'_, RuntimeState>,
    code: i32,
) -> Result<(), Error> {
    caller.data().exit_code.store(code, core::sync::atomic::Ordering::SeqCst);
    Err(Error::i32_exit(code))
}

/// Minimal poll_oneoff: only handles clock subscriptions (sleep).
///
/// The real work happens in `Fiber::dispatch(SuspendReason::Sleep)`.
/// This host fn parses the first subscription, validates it is a clock,
/// computes the delta-tick count, then traps with `SuspendReason::Sleep`
/// so that the fiber can yield to the async executor.
pub fn poll_oneoff(
    caller: Caller<'_, RuntimeState>,
    in_ptr: i32,
    out_ptr: i32,
    nsubs: i32,
    nevents_ptr: i32,
) -> Result<i32, Error> {
    use crate::wasm::suspend::SuspendReason;

    if nsubs < 1 {
        return Ok(28); // EINVAL
    }

    // WASI `__wasi_subscription_t` is 48 bytes:
    //   offset 0..8:   userdata (u64)
    //   offset 8:      tag / type (u8): 0 = CLOCK, 1 = FD_READ, 2 = FD_WRITE
    //   (padding/alignment varies — Wasm uses packed layout)
    // For wasm32, the clock variant layout is:
    //   offset 0..8:   userdata (u64)
    //   offset 8..10:  type (u16, 0 = CLOCK)
    //   offset 16..24: clock_id (u32)
    //   offset 24..32: timeout (u64, ns)
    //   offset 32..40: precision (u64)
    //   offset 40..42: flags (u16, 0 = relative, 1 = ABSTIME)
    let mut sub = [0u8; 48];
    if let Err(e) = crate::wasm::host::mem::guest_read_into(&caller, in_ptr, &mut sub) {
        return Ok(e);
    }

    // Check type byte at offset 8 (u16 LE).
    let sub_type = u16::from_le_bytes([sub[8], sub[9]]);
    if sub_type != 0 {
        // Not a clock subscription — not implemented in T1.
        return Ok(28); // EINVAL
    }

    let timeout_ns = u64::from_le_bytes([
        sub[24], sub[25], sub[26], sub[27], sub[28], sub[29], sub[30], sub[31],
    ]);
    let flags = u16::from_le_bytes([sub[40], sub[41]]);
    let abstime = flags & 0x1 != 0;

    // Timer runs at 100 Hz → 1 tick = 10_000_000 ns.
    let tick_ns: u64 = 10_000_000;
    let now_ticks = crate::timer::ticks();
    let target_ticks = if abstime {
        let abs_ticks = timeout_ns / tick_ns;
        if abs_ticks <= now_ticks { now_ticks } else { abs_ticks }
    } else {
        now_ticks.saturating_add((timeout_ns + tick_ns - 1) / tick_ns)
    };
    let delta = target_ticks.saturating_sub(now_ticks);

    // Suspend: Fiber::dispatch will await Delay::ticks(delta) then
    // write one clock event and write nevents=1.
    Err(Error::host(SuspendReason::Sleep {
        ticks: delta,
        events_ptr: out_ptr as u32,
        nevents_ptr: nevents_ptr as u32,
    }))
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("wasi_snapshot_preview1", "args_sizes_get", args_sizes_get)?
        .func_wrap("wasi_snapshot_preview1", "args_get", args_get)?
        .func_wrap("wasi_snapshot_preview1", "environ_sizes_get", environ_sizes_get)?
        .func_wrap("wasi_snapshot_preview1", "environ_get", environ_get)?
        .func_wrap("wasi_snapshot_preview1", "proc_exit", proc_exit)?
        .func_wrap("wasi_snapshot_preview1", "poll_oneoff", poll_oneoff)?;
    Ok(())
}

/// Get the wasm linear memory export from a Caller.
pub fn wasm_memory(caller: &Caller<'_, RuntimeState>) -> Result<Memory, Error> {
    caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| Error::new("no memory export"))
}

/// Write a little-endian u32 to wasm memory at `ptr`.
/// Kept for fd.rs callers; internally routes through the audited accessor.
pub fn write_u32(
    _mem: &Memory,
    caller: &mut Caller<'_, RuntimeState>,
    ptr: usize,
    val: u32,
) -> Result<(), Error> {
    crate::wasm::host::mem::guest_write_u32(caller, ptr as i32, val)
        .map_err(|e| Error::new(alloc::format!("mem write errno: {}", e)))
}

/// Read a little-endian u32 from wasm memory at `ptr`.
/// Kept for fd.rs callers; internally routes through the audited accessor.
pub fn read_u32(
    _mem: &Memory,
    caller: &Caller<'_, RuntimeState>,
    ptr: usize,
) -> Result<u32, Error> {
    let mut buf = [0u8; 4];
    crate::wasm::host::mem::guest_read_into(caller, ptr as i32, &mut buf)
        .map_err(|e| Error::new(alloc::format!("mem read errno: {}", e)))?;
    Ok(u32::from_le_bytes(buf))
}
