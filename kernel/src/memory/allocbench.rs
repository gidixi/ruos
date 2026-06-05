//! Allocator micro-benchmark (solo sotto `boot-checks`). Misura la latenza media
//! di alloc+free in cicli TSC, convertita in ns via la calibrazione del clock.
//! Usato per confrontare i prototipi di allocatore (default talc / magazine /
//! per-core talc).

use alloc::boxed::Box;
use alloc::vec::Vec;
use crate::boot::clock::read_tsc;

/// Cicli TSC → nanosecondi, mediato su `iters`.
fn cyc_to_ns(cyc: u64, iters: u64) -> u64 {
    let per_ms = crate::boot::clock::tsc_per_ms().max(1) as u128;
    // u128 intermedio: niente overflow del prodotto e nessuna perdita di precisione
    // (il divide-before-multiply perderebbe la granularità sub-ms).
    ((cyc as u128 * 1_000_000) / per_ms / (iters.max(1) as u128)) as u64
}

const SMALL_ITERS: u64 = 100_000;
const LARGE_ITERS: u64 = 256;

/// Single-core alloc/free latency su BSP. Stampa il marker greppabile dal test.
pub fn run_single_core() {
    let mut acc: u64 = 0;
    // Warm-up (non cronometrato): pre-tocca i path di alloc/free.
    for _ in 0..1_000 { let b = Box::new(0u64); core::hint::black_box(&b); drop(b); }
    let t0 = read_tsc();
    for _ in 0..SMALL_ITERS {
        let b = Box::new(0xA5u64);
        acc = acc.wrapping_add(*b);
        core::hint::black_box(&b);
        drop(b);
    }
    let small_cyc = read_tsc().saturating_sub(t0);

    for _ in 0..8 { let v: Vec<u8> = Vec::with_capacity(1024 * 1024); core::hint::black_box(&v); drop(v); }
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

use core::sync::atomic::{AtomicU64, Ordering};

/// Sink globale: impedisce la dead-code-elimination dei job.
static BENCH_SINK: AtomicU64 = AtomicU64::new(0);

/// Job di contesa: alloca+libera molti blocchi piccoli, ritorna un checksum.
/// Firma `JobFn` del pool. `input` ignorato.
fn alloc_churn_job(_input: &[u8]) -> u64 {
    let mut acc: u64 = 0;
    for i in 0..50_000u64 {
        let b = Box::new(i);
        acc = acc.wrapping_add(*b);
        core::hint::black_box(&b);
        drop(b);
    }
    BENCH_SINK.fetch_add(acc, Ordering::Relaxed);
    acc
}

/// Sottomette fino a un job di churn per core online, li attende (con il BSP che
/// fa da worker di fallback), misura il wall-time TSC e i core distinti che li
/// hanno eseguiti.
pub fn run_multicore() {
    static EMPTY_INPUT: [u8; 0] = [];
    let n = crate::cpu::cpus_online().max(1) as usize;
    let want = n.min(crate::smp::pool::MAX_JOBS);
    let mut ids: Vec<usize> = Vec::with_capacity(want);
    let t0 = read_tsc();
    for _ in 0..want {
        match crate::smp::pool::submit(alloc_churn_job, &EMPTY_INPUT) {
            Some(id) => ids.push(id),
            None => break,
        }
    }
    let mut seen = alloc::vec![false; ids.len()];
    let mut cores_mask: u32 = 0;
    let mut done = 0usize;
    while done < ids.len() {
        // Il BSP aiuta come worker: esegue un job inline se disponibile.
        if let Some(slot) = crate::smp::pool::take() {
            crate::smp::pool::run_slot(slot, crate::cpu::cpu_id());
        }
        for (k, &id) in ids.iter().enumerate() {
            if !seen[k] {
                if let Some((_r, cpu)) = crate::smp::pool::poll_done(id) {
                    seen[k] = true;
                    cores_mask |= 1u32 << (cpu & 31);
                    done += 1;
                }
            }
        }
    }
    let total_cyc = read_tsc().saturating_sub(t0);
    let cores = cores_mask.count_ones();
    crate::binfo!(
        "allocbench",
        "multi cores={} total_ns={} per_job={} jobs={} sink=0x{:X}",
        cores,
        cyc_to_ns(total_cyc, 1),
        cyc_to_ns(total_cyc, ids.len().max(1) as u64),
        ids.len(),
        BENCH_SINK.load(Ordering::Relaxed)
    );
}
