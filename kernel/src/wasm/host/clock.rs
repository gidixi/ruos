//! WASIX clock host fns. Backed by ruos's TICKS atomic (100 Hz).

use wasmi::{Caller, Linker, Error};
use crate::wasm::state::RuntimeState;
use crate::wasm::host::lifecycle::wasm_memory;

const TICK_NS: u64 = 10_000_000; // 100 Hz → 10 ms per tick → 10^7 ns

pub fn clock_time_get(
    mut caller: Caller<'_, RuntimeState>,
    _clock_id: i32,
    _precision: i64,
    time_ptr: i32,
) -> Result<i32, Error> {
    let ticks = crate::timer::ticks();
    let nanos: u64 = ticks * TICK_NS;
    let mem = wasm_memory(&caller)?;
    mem.write(&mut caller, time_ptr as usize, &nanos.to_le_bytes())
        .map_err(|_| Error::i32_exit(-1))?;
    Ok(0)
}

pub fn clock_res_get(
    mut caller: Caller<'_, RuntimeState>,
    _clock_id: i32,
    res_ptr: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    mem.write(&mut caller, res_ptr as usize, &TICK_NS.to_le_bytes())
        .map_err(|_| Error::i32_exit(-1))?;
    Ok(0)
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("wasi_snapshot_preview1", "clock_time_get", clock_time_get)?
        .func_wrap("wasi_snapshot_preview1", "clock_res_get", clock_res_get)?;
    Ok(())
}
