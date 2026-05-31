//! WASI random_get host fn backed by the RDRAND-seeded ChaCha20 CSPRNG in `crate::rng`.

use wasmi::{Caller, Linker, Error};
use crate::wasm::state::RuntimeState;

pub fn random_get(
    mut caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
) -> Result<i32, Error> {
    if buf_len < 0 { return Ok(crate::wasm::host::mem::EINVAL); }
    let mut tmp = [0u8; 256];
    let mut remaining = buf_len as usize;
    let mut offset = buf_ptr;
    while remaining > 0 {
        let n = remaining.min(tmp.len());
        crate::rng::fill(&mut tmp[..n]);
        if let Err(e) = crate::wasm::host::mem::guest_write(&mut caller, offset, &tmp[..n]) {
            return Ok(e);
        }
        offset = offset.wrapping_add(n as i32);
        remaining -= n;
    }
    Ok(0)
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker.func_wrap("wasi_snapshot_preview1", "random_get", random_get)?;
    Ok(())
}
