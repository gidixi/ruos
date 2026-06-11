//! Host fns `ruos.unit_*` / `timer_list`: ABI del CLI `unitctl`. TSV
//! (vedi `service::list_tsv`), stesso pattern buffer+used di `service.rs`:
//! buffer troppo piccolo → 8 (ENOBUFS) con la size richiesta in `used`.
use wasmi::{Caller, Linker, Error};
use alloc::string::String;

use crate::wasm::state::RuntimeState;

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("ruos", "unit_list",   ruos_unit_list)?
        .func_wrap("ruos", "unit_status", ruos_unit_status)?
        .func_wrap("ruos", "unit_start",  ruos_unit_start)?
        .func_wrap("ruos", "unit_stop",   ruos_unit_stop)?
        .func_wrap("ruos", "unit_enable", ruos_unit_enable)?
        .func_wrap("ruos", "timer_list",  ruos_timer_list)?
        .func_wrap("ruos", "unit_reload", ruos_unit_reload)?;
    Ok(())
}

fn write_text(
    caller: &mut Caller<'_, RuntimeState>,
    text: &str, buf_ptr: i32, buf_len: i32, used_ptr: i32,
) -> i32 {
    let bytes = text.as_bytes();
    if let Err(e) = crate::wasm::host::mem::guest_write_u32(caller, used_ptr, bytes.len() as u32) {
        return e;
    }
    if (buf_len as usize) < bytes.len() { return 8; } // ENOBUFS
    if let Err(e) = crate::wasm::host::mem::guest_write(caller, buf_ptr, bytes) { return e; }
    0
}

fn read_name(caller: &Caller<'_, RuntimeState>, ptr: i32, len: i32) -> Result<String, Error> {
    let buf = crate::wasm::host::mem::guest_read(caller, ptr, len)
        .map_err(|_| Error::i32_exit(-1))?;
    core::str::from_utf8(&buf).map(|s| s.into()).map_err(|_| Error::i32_exit(-1))
}

/// unit_list(buf, len, used) -> errno
pub fn ruos_unit_list(
    mut caller: Caller<'_, RuntimeState>, buf_ptr: i32, buf_len: i32, used_ptr: i32,
) -> Result<i32, Error> {
    let text = crate::service::list_tsv();
    Ok(write_text(&mut caller, &text, buf_ptr, buf_len, used_ptr))
}

/// unit_status(name_ptr, name_len, buf, len, used) -> errno (1 NotFound)
pub fn ruos_unit_status(
    mut caller: Caller<'_, RuntimeState>,
    name_ptr: i32, name_len: i32, buf_ptr: i32, buf_len: i32, used_ptr: i32,
) -> Result<i32, Error> {
    let name = read_name(&caller, name_ptr, name_len)?;
    match crate::service::status_tsv(&name) {
        Some(text) => Ok(write_text(&mut caller, &text, buf_ptr, buf_len, used_ptr)),
        None => Ok(1), // NotFound
    }
}

/// unit_start(name_ptr, name_len) -> errno (queue, async)
pub fn ruos_unit_start(
    caller: Caller<'_, RuntimeState>, name_ptr: i32, name_len: i32,
) -> Result<i32, Error> {
    let name = read_name(&caller, name_ptr, name_len)?;
    Ok(match crate::service::start(&name) { Ok(()) => 0, Err(e) => e.errno() })
}

/// unit_stop(name_ptr, name_len) -> errno (cooperativo, best-effort)
pub fn ruos_unit_stop(
    caller: Caller<'_, RuntimeState>, name_ptr: i32, name_len: i32,
) -> Result<i32, Error> {
    let name = read_name(&caller, name_ptr, name_len)?;
    Ok(match crate::service::stop(&name) { Ok(()) => 0, Err(e) => e.errno() })
}

/// unit_enable(name_ptr, name_len, on) -> errno (+ persistenza su file)
pub fn ruos_unit_enable(
    caller: Caller<'_, RuntimeState>, name_ptr: i32, name_len: i32, on: i32,
) -> Result<i32, Error> {
    let name = read_name(&caller, name_ptr, name_len)?;
    Ok(match crate::service::set_enabled(&name, on != 0) { Ok(()) => 0, Err(e) => e.errno() })
}

/// timer_list(buf, len, used) -> errno
pub fn ruos_timer_list(
    mut caller: Caller<'_, RuntimeState>, buf_ptr: i32, buf_len: i32, used_ptr: i32,
) -> Result<i32, Error> {
    let text = crate::service::timers_tsv();
    Ok(write_text(&mut caller, &text, buf_ptr, buf_len, used_ptr))
}

/// unit_reload() -> errno (sempre 0: il parse avviene async nel dispatcher,
/// gli errori finiscono nel klog)
pub fn ruos_unit_reload(_caller: Caller<'_, RuntimeState>) -> Result<i32, Error> {
    use crate::service::{SERVICE_QUEUE, UnitReq};
    SERVICE_QUEUE.pending.lock().push_back(UnitReq::Reload);
    if let Some(w) = SERVICE_QUEUE.worker_waker.lock().take() { w.wake(); }
    Ok(0)
}
