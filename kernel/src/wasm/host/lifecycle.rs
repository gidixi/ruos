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
    let mem = wasm_memory(&caller)?;
    write_u32(&mem, &mut caller, argc_ptr as usize, argc)?;
    write_u32(&mem, &mut caller, argv_buf_size_ptr as usize, argv_buf)?;
    Ok(0)
}

pub fn args_get(
    _caller: Caller<'_, RuntimeState>,
    _argv_ptr: i32,
    _argv_buf_ptr: i32,
) -> Result<i32, Error> {
    Ok(0)
}

pub fn environ_sizes_get(
    mut caller: Caller<'_, RuntimeState>,
    environc_ptr: i32,
    environ_buf_size_ptr: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    write_u32(&mem, &mut caller, environc_ptr as usize, 0)?;
    write_u32(&mem, &mut caller, environ_buf_size_ptr as usize, 0)?;
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

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("wasi_snapshot_preview1", "args_sizes_get", args_sizes_get)?
        .func_wrap("wasi_snapshot_preview1", "args_get", args_get)?
        .func_wrap("wasi_snapshot_preview1", "environ_sizes_get", environ_sizes_get)?
        .func_wrap("wasi_snapshot_preview1", "environ_get", environ_get)?
        .func_wrap("wasi_snapshot_preview1", "proc_exit", proc_exit)?;
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
pub fn write_u32(
    mem: &Memory,
    caller: &mut Caller<'_, RuntimeState>,
    ptr: usize,
    val: u32,
) -> Result<(), Error> {
    let bytes = val.to_le_bytes();
    mem.write(caller, ptr, &bytes)
        .map_err(|e| Error::new(alloc::format!("mem write: {}", e)))
}

/// Read a little-endian u32 from wasm memory at `ptr`.
pub fn read_u32(
    mem: &Memory,
    caller: &Caller<'_, RuntimeState>,
    ptr: usize,
) -> Result<u32, Error> {
    let mut buf = [0u8; 4];
    mem.read(caller, ptr, &mut buf)
        .map_err(|e| Error::new(alloc::format!("mem read: {}", e)))?;
    Ok(u32::from_le_bytes(buf))
}
