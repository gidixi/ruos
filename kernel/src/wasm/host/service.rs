//! Host fns for the userspace `service` tool. Bridges the kernel
//! registry in `crate::service` to wasmi-side ABI calls.
//!
//! Serialization format for `list` / `status` is plain ASCII TSV, one
//! line per entry:
//!
//! ```text
//! name<TAB>status<TAB>pid<TAB>runs<TAB>path\n
//! ```
//!
//! Keeping the format text-only avoids a separate decode path in
//! userspace — the CLI can `str::split('\t')` directly.

use wasmi::{Caller, Linker, Error};
use alloc::string::String;
use core::fmt::Write as _;

use crate::wasm::state::RuntimeState;

/// Format one entry into the buffer. Trailing newline included.
fn format_entry(out: &mut String, info: &crate::service::ServiceInfo) {
    let pid = info.pid.map(|p| alloc::format!("{}", p)).unwrap_or_else(|| "-".into());
    let _ = writeln!(
        out,
        "{}\t{}\t{}\t{}\t{}",
        info.name, info.status, pid, info.runs, info.path,
    );
}

/// ruos_service_list(buf_ptr, buf_len, used_ptr) -> errno
///
/// Writes the whole registry. On buffer-too-small returns 8 (ENOBUFS)
/// and still writes the required size at `used_ptr` so the caller can
/// resize and retry. On success returns 0.
pub fn ruos_service_list(
    mut caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
    used_ptr: i32,
) -> Result<i32, Error> {
    let mut text = String::new();
    for info in crate::service::list() {
        format_entry(&mut text, &info);
    }
    let bytes = text.as_bytes();
    let need = bytes.len() as u32;
    if let Err(e) = crate::wasm::host::mem::guest_write_u32(&mut caller, used_ptr, need) {
        return Ok(e);
    }
    if (buf_len as usize) < bytes.len() {
        return Ok(8); // ENOBUFS
    }
    if let Err(e) = crate::wasm::host::mem::guest_write(&mut caller, buf_ptr, bytes) {
        return Ok(e);
    }
    Ok(0)
}

/// ruos_service_start(name_ptr, name_len) -> errno
///
/// Returns the `ServiceError::errno()` mapping (1 NotFound, 2 Already,
/// 3 NotSupported, 99 Internal) or 0 on success. The actual fiber spawn
/// happens asynchronously in `executor::service_dispatcher_task`.
pub fn ruos_service_start(
    caller: Caller<'_, RuntimeState>,
    name_ptr: i32,
    name_len: i32,
) -> Result<i32, Error> {
    let name = read_name(&caller, name_ptr, name_len)?;
    match crate::service::start(&name) {
        Ok(()) => Ok(0),
        Err(e) => Ok(e.errno()),
    }
}

/// ruos_service_status(name_ptr, name_len, buf_ptr, buf_len, used_ptr) -> errno
///
/// Same line format as `_list` but for a single entry. Returns 1 (NotFound)
/// if no such service; 8 (ENOBUFS) if the buffer was too small; 0 OK.
pub fn ruos_service_status(
    mut caller: Caller<'_, RuntimeState>,
    name_ptr: i32,
    name_len: i32,
    buf_ptr: i32,
    buf_len: i32,
    used_ptr: i32,
) -> Result<i32, Error> {
    let name = read_name(&caller, name_ptr, name_len)?;
    let info = match crate::service::status(&name) {
        Some(i) => i,
        None    => return Ok(1), // NotFound
    };
    let mut text = String::new();
    format_entry(&mut text, &info);
    let bytes = text.as_bytes();
    let need = bytes.len() as u32;
    if let Err(e) = crate::wasm::host::mem::guest_write_u32(&mut caller, used_ptr, need) {
        return Ok(e);
    }
    if (buf_len as usize) < bytes.len() {
        return Ok(8); // ENOBUFS
    }
    if let Err(e) = crate::wasm::host::mem::guest_write(&mut caller, buf_ptr, bytes) {
        return Ok(e);
    }
    Ok(0)
}

fn read_name(
    caller: &Caller<'_, RuntimeState>,
    name_ptr: i32,
    name_len: i32,
) -> Result<String, Error> {
    let buf = crate::wasm::host::mem::guest_read(caller, name_ptr, name_len)
        .map_err(|_| Error::i32_exit(-1))?;
    core::str::from_utf8(&buf)
        .map(|s| s.into())
        .map_err(|_| Error::i32_exit(-1))
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("ruos", "service_list",   ruos_service_list)?
        .func_wrap("ruos", "service_start",  ruos_service_start)?
        .func_wrap("ruos", "service_status", ruos_service_status)?;
    Ok(())
}
