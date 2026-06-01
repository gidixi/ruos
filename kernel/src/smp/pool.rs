//! Kernel compute offload pool. A fixed array of job slots + a queue of slot
//! ids (IrqMutex). The BSP `submit`s pure-CPU jobs; AP worker loops `take` and
//! run them on their core, then `complete`. No I/O, no shared mutable state in
//! a job — pure functions over `'static` immutable input.

use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use alloc::collections::VecDeque;
use crate::sync::IrqMutex;

/// Max in-flight jobs.
pub const MAX_JOBS: usize = 64;

// Slot states.
const EMPTY: u8 = 0;
const QUEUED: u8 = 1;
const RUNNING: u8 = 2;
const DONE: u8 = 3;

/// A pure-CPU job: `fn(&[u8]) -> u64`. No captures, no I/O, no blocking.
pub type JobFn = fn(&[u8]) -> u64;

struct JobSlot {
    state: AtomicU8,
    work: AtomicUsize,    // JobFn as usize (fn pointer)
    input_ptr: AtomicUsize,
    input_len: AtomicUsize,
    result: AtomicU64,
    ran_on: AtomicU32,    // cpu_id that executed it
}

impl JobSlot {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(EMPTY),
            work: AtomicUsize::new(0),
            input_ptr: AtomicUsize::new(0),
            input_len: AtomicUsize::new(0),
            result: AtomicU64::new(0),
            ran_on: AtomicU32::new(u32::MAX),
        }
    }
}

static SLOTS: [JobSlot; MAX_JOBS] = {
    const S: JobSlot = JobSlot::new();
    [S; MAX_JOBS]
};

/// Queue of QUEUED slot ids, in submission order.
static QUEUE: IrqMutex<VecDeque<usize>> = IrqMutex::new(VecDeque::new());

/// Submit a pure-CPU job. `input` must be `'static` (lives until the job is
/// done). Returns the slot id, or None if the pool is full.
pub fn submit(work: JobFn, input: &'static [u8]) -> Option<usize> {
    for (id, slot) in SLOTS.iter().enumerate() {
        if slot.state.compare_exchange(EMPTY, QUEUED, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            slot.work.store(work as usize, Ordering::SeqCst);
            slot.input_ptr.store(input.as_ptr() as usize, Ordering::SeqCst);
            slot.input_len.store(input.len(), Ordering::SeqCst);
            slot.ran_on.store(u32::MAX, Ordering::SeqCst);
            QUEUE.lock().push_back(id);
            // Wake any sleeping AP worker cores so one picks up the job.
            crate::apic::lapic::send_ipi_all_but_self(crate::idt::VEC_WAKE);
            return Some(id);
        }
    }
    None
}

/// True if the job queue currently holds no work. Used by AP worker loops to
/// decide between running jobs and sleeping (`hlt`).
pub fn is_empty() -> bool {
    QUEUE.lock().is_empty()
}

/// Take a QUEUED slot id off the queue (called by AP workers and the BSP
/// fallback). CAS QUEUED->RUNNING; returns the id if claimed.
pub fn take() -> Option<usize> {
    let id = QUEUE.lock().pop_front()?;
    if SLOTS[id].state.compare_exchange(QUEUED, RUNNING, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        Some(id)
    } else {
        None
    }
}

/// Run the slot's job on the current core and mark it DONE.
pub fn run_slot(id: usize, cpu: u32) {
    let slot = &SLOTS[id];
    let work_addr = slot.work.load(Ordering::SeqCst);
    let ptr = slot.input_ptr.load(Ordering::SeqCst) as *const u8;
    let len = slot.input_len.load(Ordering::SeqCst);
    // SAFETY: input was a `'static [u8]` passed to submit; ptr/len reconstruct
    // the same slice, valid until the BSP frees the slot after DONE.
    let input: &[u8] = unsafe { core::slice::from_raw_parts(ptr, len) };
    // SAFETY: work_addr is a `JobFn` (fn(&[u8])->u64) stored by submit as a
    // raw usize via `work as usize`. We round-trip through `*const ()` because
    // Rust may reject a direct usize→fn-pointer transmute; fn pointers are
    // thin (one machine word) on this target, so the usize round-trip is sound.
    let work: JobFn = unsafe { core::mem::transmute::<*const (), JobFn>(work_addr as *const ()) };
    let result = work(input);
    slot.result.store(result, Ordering::SeqCst);
    slot.ran_on.store(cpu, Ordering::SeqCst);
    slot.state.store(DONE, Ordering::SeqCst);
}

/// If slot `id` is DONE, return (result, ran_on cpu) and free the slot.
/// Returns None if not done yet.
pub fn poll_done(id: usize) -> Option<(u64, u32)> {
    let slot = &SLOTS[id];
    if slot.state.load(Ordering::SeqCst) == DONE {
        let r = slot.result.load(Ordering::SeqCst);
        let c = slot.ran_on.load(Ordering::SeqCst);
        slot.state.store(EMPTY, Ordering::SeqCst);
        Some((r, c))
    } else {
        None
    }
}
