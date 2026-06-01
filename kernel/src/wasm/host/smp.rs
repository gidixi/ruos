//! Host fn `ruos_smp_bench`: run N identical pure-CPU hash jobs via the SMP
//! pool (parallel across APs) and inline on the BSP (sequential), time both,
//! and report the speedup + the set of cpu_ids that ran the parallel jobs.

use wasmi::{Caller, Linker, Error};
use alloc::string::String;
use core::fmt::Write as _;
use crate::wasm::state::RuntimeState;

/// Iterations per job — tuned so one job is ~tens of ms (measurable, not too
/// long for tests).
const ITERS: u64 = 4_000_000;

/// Fixed input buffer the jobs hash over. `'static` so it can be handed to the
/// pool. Arbitrary but constant (deterministic result).
static JOB_INPUT: [u8; 64] = [0x5a; 64];

/// Pure-CPU job: heavy integer-mixing hash over `input`. No I/O, no shared
/// state — safe to run on any core.
fn hash_job(input: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut i: u64 = 0;
    while i < ITERS {
        let mut k = 0usize;
        while k < input.len() {
            h = (h ^ input[k] as u64).wrapping_mul(0x100000001b3);
            k += 1;
        }
        h = h.rotate_left(13).wrapping_add(0x9e3779b97f4a7c15);
        i += 1;
    }
    h
}

/// ruos_smp_bench(buf_ptr, buf_len, used_ptr) -> errno.
/// Writes a one-line ASCII report to the guest buffer:
///   "parallel=Xms sequential=Yms speedup=Z.ZZx cores=[a,b,c]"
pub fn ruos_smp_bench(
    caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
    used_ptr: i32,
) -> Result<i32, Error> {
    let n_aps = crate::cpu::cpus_online();
    let n_jobs: usize = if n_aps == 0 { 4 } else { (n_aps as usize).min(8) * 2 };

    // --- Parallel: submit all jobs, then drain results. ---
    let t0 = crate::boot::clock::elapsed_ms();
    let mut ids: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
    for _ in 0..n_jobs {
        match crate::smp::pool::submit(hash_job, &JOB_INPUT) {
            Some(id) => ids.push(id),
            None => break,
        }
    }
    // No APs (1 CPU): drain inline on the BSP so we don't deadlock waiting.
    if n_aps == 0 {
        while let Some(slot) = crate::smp::pool::take() {
            crate::smp::pool::run_slot(slot, crate::cpu::cpu_id());
        }
    }
    // Collect results + the cores that ran them.
    let mut cores: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
    for &id in &ids {
        loop {
            if let Some((_r, c)) = crate::smp::pool::poll_done(id) {
                if !cores.contains(&c) { cores.push(c); }
                break;
            }
            core::hint::spin_loop();
        }
    }
    let parallel_ms = crate::boot::clock::elapsed_ms().saturating_sub(t0);

    // --- Sequential: run the same n_jobs inline on the BSP. ---
    let t1 = crate::boot::clock::elapsed_ms();
    let mut acc: u64 = 0;
    for _ in 0..n_jobs {
        acc = acc.wrapping_add(hash_job(&JOB_INPUT));
    }
    let sequential_ms = crate::boot::clock::elapsed_ms().saturating_sub(t1);
    let _ = acc;

    // --- Report. ---
    let speedup_x100 = if parallel_ms == 0 { 0 } else { sequential_ms * 100 / parallel_ms };
    let mut s = String::new();
    let _ = write!(s, "parallel={}ms sequential={}ms speedup={}.{:02}x cores=[",
        parallel_ms, sequential_ms, speedup_x100 / 100, speedup_x100 % 100);
    cores.sort_unstable();
    for (i, c) in cores.iter().enumerate() {
        if i > 0 { let _ = write!(s, ","); }
        let _ = write!(s, "{}", c);
    }
    let _ = write!(s, "]");

    crate::wasm::host::sysinfo::write_bytes_and_len(caller, buf_ptr, buf_len, used_ptr, s.as_bytes())
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("ruos", "smp_bench", ruos_smp_bench)?;
    Ok(())
}
