//! Allocator micro-benchmark (solo sotto `boot-checks`). Misura la latenza media
//! di alloc+free in cicli TSC, convertita in ns via la calibrazione del clock.
//! Usato per confrontare i prototipi di allocatore (default talc / magazine /
//! per-core talc).

use alloc::boxed::Box;
use alloc::vec::Vec;
use crate::boot::clock::read_tsc;

/// Cicli TSC → nanosecondi, mediato su `iters`.
fn cyc_to_ns(cyc: u64, iters: u64) -> u64 {
    let per_ms = crate::boot::clock::tsc_per_ms().max(1);
    cyc.saturating_mul(1_000_000) / per_ms / iters.max(1)
}

const SMALL_ITERS: u64 = 100_000;
const LARGE_ITERS: u64 = 256;

/// Single-core alloc/free latency su BSP. Stampa il marker greppabile dal test.
pub fn run_single_core() {
    let mut acc: u64 = 0;
    let t0 = read_tsc();
    for _ in 0..SMALL_ITERS {
        let b = Box::new(0xA5u64);
        acc = acc.wrapping_add(&*b as *const u64 as u64);
        core::hint::black_box(&b);
        drop(b);
    }
    let small_cyc = read_tsc().saturating_sub(t0);

    let t1 = read_tsc();
    for _ in 0..LARGE_ITERS {
        let mut v: Vec<u8> = Vec::with_capacity(1024 * 1024);
        v.push((acc & 0xFF) as u8);
        core::hint::black_box(&v);
        drop(v);
    }
    let large_cyc = read_tsc().saturating_sub(t1);

    crate::binfo!(
        "allocbench",
        "single small_ns={} large_ns={} iters={} acc=0x{:X}",
        cyc_to_ns(small_cyc, SMALL_ITERS),
        cyc_to_ns(large_cyc, LARGE_ITERS),
        SMALL_ITERS, acc
    );
}
