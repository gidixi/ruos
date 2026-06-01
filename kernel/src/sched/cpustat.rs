//! Per-core busy/idle TSC accounting.
//!
//! ruos has no preemptive scheduler: on each core, time is either spent
//! *executing* (the BSP polling the executor, an AP running a pool job) or
//! *halted*. We accumulate raw TSC cycles into two monotonic counters per
//! core; `rtop` reads two snapshots and computes `busy / (busy + idle)`.
//!
//! Counters are best-effort telemetry: a missed or skewed sample only nudges
//! a percentage, never affects correctness.

use core::sync::atomic::{AtomicU64, Ordering};
use crate::cpu::MAX_CPUS;

pub struct CoreStat {
    pub busy_tsc: AtomicU64,
    pub idle_tsc: AtomicU64,
}

impl CoreStat {
    const fn new() -> Self {
        Self { busy_tsc: AtomicU64::new(0), idle_tsc: AtomicU64::new(0) }
    }
}

#[allow(clippy::declare_interior_mutable_const)]
const ZERO: CoreStat = CoreStat::new();
static CORE: [CoreStat; MAX_CPUS] = [ZERO; MAX_CPUS];

/// Add `delta` busy cycles to core `cpu`. Out-of-range `cpu` is ignored.
pub fn add_busy(cpu: usize, delta: u64) {
    if let Some(c) = CORE.get(cpu) {
        c.busy_tsc.fetch_add(delta, Ordering::Relaxed);
    }
}

/// Add `delta` idle (halted) cycles to core `cpu`. Out-of-range ignored.
pub fn add_idle(cpu: usize, delta: u64) {
    if let Some(c) = CORE.get(cpu) {
        c.idle_tsc.fetch_add(delta, Ordering::Relaxed);
    }
}

/// Read `(busy, idle)` for core `cpu`. Returns `(0, 0)` if out of range.
pub fn read(cpu: usize) -> (u64, u64) {
    match CORE.get(cpu) {
        Some(c) => (c.busy_tsc.load(Ordering::Relaxed), c.idle_tsc.load(Ordering::Relaxed)),
        None => (0, 0),
    }
}
