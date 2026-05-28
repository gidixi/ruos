//! WASIX random host fn. Weak xorshift PRNG seeded from TICKS.
//! Step 14 (SSH) replaces with RDRAND-backed CSPRNG.

use core::sync::atomic::{AtomicU64, Ordering};
use wasmi::{Caller, Linker, Error};
use crate::wasm::state::RuntimeState;
use crate::wasm::host::lifecycle::wasm_memory;

static STATE: AtomicU64 = AtomicU64::new(0);

fn ensure_seeded() {
    if STATE.load(Ordering::Relaxed) == 0 {
        let t = crate::timer::ticks();
        let seed = t.wrapping_mul(0x2545F4914F6CDD1D) ^ 0xDEADBEEFCAFEBABE;
        STATE.store(seed | 1, Ordering::Relaxed);
    }
}

fn next() -> u64 {
    ensure_seeded();
    let mut x = STATE.load(Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    x
}

pub fn random_get(
    mut caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    let mut remaining = buf_len as usize;
    let mut offset = buf_ptr as usize;
    while remaining > 0 {
        let chunk = next().to_le_bytes();
        let n = remaining.min(8);
        mem.write(&mut caller, offset, &chunk[..n])
            .map_err(|_| Error::i32_exit(-1))?;
        offset += n;
        remaining -= n;
    }
    Ok(0)
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker.func_wrap("wasi_snapshot_preview1", "random_get", random_get)?;
    Ok(())
}
