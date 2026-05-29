//! Custom (non-WASIX) host fns: ruos_exec + ruos_readdir.

use wasmi::{Caller, Linker, Error};
use alloc::string::ToString;
use alloc::vec::Vec;
use crate::wasm::state::RuntimeState;
use crate::wasm::host::lifecycle::wasm_memory;
use crate::wasm::suspend::SuspendReason;

pub fn ruos_exec(
    caller: Caller<'_, RuntimeState>,
    path_ptr: i32,
    path_len: i32,
    argv_ptr: i32,
    argv_len: i32,
    exit_code_ptr: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    let mut path_buf = alloc::vec![0u8; path_len as usize];
    mem.read(&caller, path_ptr as usize, &mut path_buf)
        .map_err(|_| Error::i32_exit(-1))?;
    let path = core::str::from_utf8(&path_buf)
        .map_err(|_| Error::i32_exit(-1))?
        .to_string();
    let mut argv_blob = alloc::vec![0u8; argv_len as usize];
    mem.read(&caller, argv_ptr as usize, &mut argv_blob)
        .map_err(|_| Error::i32_exit(-1))?;
    let argv = decode_argv(&argv_blob).unwrap_or_default();
    Err(Error::host(SuspendReason::Exec {
        path,
        argv,
        exit_code_ptr: exit_code_ptr as u32,
    }))
}

fn decode_argv(blob: &[u8]) -> Option<Vec<Vec<u8>>> {
    if blob.len() < 4 { return None; }
    let count = u32::from_le_bytes([blob[0], blob[1], blob[2], blob[3]]) as usize;
    let mut out = Vec::with_capacity(count);
    let table_start = 4usize;
    let table_end = table_start.checked_add(count.checked_mul(8)?)?;
    if blob.len() < table_end { return None; }
    for i in 0..count {
        let off = table_start + i * 8;
        let offset = u32::from_le_bytes([blob[off], blob[off+1], blob[off+2], blob[off+3]]) as usize;
        let length = u32::from_le_bytes([blob[off+4], blob[off+5], blob[off+6], blob[off+7]]) as usize;
        let end = offset.checked_add(length)?;
        if blob.len() < end { return None; }
        out.push(blob[offset..end].to_vec());
    }
    Some(out)
}

pub fn ruos_readdir(
    caller: Caller<'_, RuntimeState>,
    path_ptr: i32,
    path_len: i32,
    buf_ptr: i32,
    buf_len: i32,
    nread_ptr: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    let mut path_buf = alloc::vec![0u8; path_len as usize];
    mem.read(&caller, path_ptr as usize, &mut path_buf)
        .map_err(|_| Error::i32_exit(-1))?;
    let path = core::str::from_utf8(&path_buf)
        .map_err(|_| Error::i32_exit(-1))?
        .to_string();
    Err(Error::host(SuspendReason::ReadDir {
        path,
        buf_ptr: buf_ptr as u32,
        buf_len: buf_len as usize,
        nread_ptr: nread_ptr as u32,
    }))
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("ruos", "exec", ruos_exec)?
        .func_wrap("ruos", "readdir", ruos_readdir)?;
    Ok(())
}
