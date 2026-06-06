//! Tick-based `Delay` future for ruos.
//!
//! Each `Delay` future, when polled, registers its waker into a **per-core**
//! slot list along with a target `TICKS` value and a unique generation tag.
//! The timer ISR scans the list on every fire and wakes any slot whose target
//! has been reached. The future's `Drop` impl clears the slot to handle
//! cancellation.
//!
//! ## Per-core lists (`PER_CORE_DELAYS`)
//!
//! We maintain one `Mutex<[Option<Slot>; SLOTS]>` per logical CPU (up to
//! `MAX_CPUS = 16`). This replaces the old single global `SLOTS_LIST`.
//!
//! **Single-writer-per-slot invariant (spec §2 inv. 5):** a `Delay` future is
//! always polled on the same core for its entire lifetime — the cooperative
//! executor never migrates tasks. Therefore `free_slot` and the task-side path
//! in `poll` always access the same core's list (`my_list()` via `cpu_id()`),
//! and the timer ISR drains `PER_CORE_DELAYS[cpu]` where `cpu` is the firing
//! core's id (`timer_tick_core(now, cpu)` called from `timer_handler`). No
//! cross-core writer ever touches a given core's slot list.
//!
//! The generation tag closes an ABA race that would otherwise be triggered as
//! soon as two `Delay`-using tasks coexist: after the ISR consumes a slot, the
//! future's `self.slot` still points at the index; a *different* `Delay` may
//! grab the same index before the woken task runs `free_slot()`. Matching the
//! stored generation against the occupant's prevents the orphaned `free_slot`
//! from wiping the new occupant.
//!
//! `GEN_COUNTER` stays a single global `AtomicU64`. It only needs to be
//! monotonically unique (the gen tag is matched *within* one core's list), but
//! global uniqueness is a harmless superset and avoids a per-core counter array.
//!
//! Task-side accesses are wrapped in `without_interrupts` to avoid a deadlock if
//! the timer IRQ fires while the lock is held. The ISR-side uses `try_lock` and
//! defers wakes to the next tick if the lock is contended (max 10 ms latency at
//! our 100 Hz tick rate — accepted trade-off).

use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicU64, Ordering};
use core::task::{Context, Poll, Waker};
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;

// One slot per concurrently-pending `Delay`. Background tasks (net poll, tick,
// pty watchdog, ssh dispatcher) hold ~4-5 continuously; each interactive app
// that races a read against a timer (rtop's auto-refresh, nano) holds one more.
// 8 was too tight — two rtop sessions plus background tasks exhausted it and the
// old code PANICKED (→ kernel halt → "system frozen"). 64 leaves wide headroom;
// exhaustion is now non-fatal anyway (see `poll`).
const SLOTS: usize = 64;

struct Slot {
    target: u64,
    waker: Waker,
    gen: u64,
}

// One wake list PER CORE; each `Delay` future occupies at most one slot
// at a time (idx + gen is recorded in the future itself).
const NONE_SLOT: Option<Slot> = None; // `Slot` isn't Copy (holds a Waker)

struct DelayList(Mutex<[Option<Slot>; SLOTS]>);
impl DelayList {
    const fn new() -> Self { Self(Mutex::new([NONE_SLOT; SLOTS])) }
}
// SAFETY: `Mutex` is Send+Sync; we assert Sync for `DelayList` so it can live in
// a static array. Access is partitioned per-core by construction.
unsafe impl Sync for DelayList {}

static PER_CORE_DELAYS: [DelayList; crate::cpu::MAX_CPUS] = {
    const L: DelayList = DelayList::new();
    [L; crate::cpu::MAX_CPUS]
};

/// Return the Delay slot list for the CURRENT core. Uses `cpu_id()` which is
/// O(1) (RDTSCP fast path or LAPIC fallback). Must only be called from task
/// context (not ISR); the ISR uses `timer_tick_core(now, cpu)` with the
/// already-determined cpu id.
#[inline]
fn my_list() -> &'static Mutex<[Option<Slot>; SLOTS]> {
    &PER_CORE_DELAYS[crate::cpu::cpu_id() as usize].0
}

// Monotonic per-registration counter. u64 means we wrap after ~5.8 * 10^11
// years at 1 GHz — non-issue.
static GEN_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Future that resolves once `TICKS` has advanced by `n` from creation.
pub struct Delay {
    target: u64,
    /// `Some((idx, gen))` once registered; `None` before first
    /// `Pending`-returning poll, or after `free_slot` runs.
    slot: Option<(usize, u64)>,
}

impl Delay {
    /// Construct a `Delay` that resolves after `n` timer ticks from
    /// the moment this is called (NOT the moment of first poll).
    pub fn ticks(n: u64) -> Self {
        let now = crate::timer::ticks();
        Delay { target: now.saturating_add(n), slot: None }
    }

    /// Clear our slot registration if any. Generation-matched: if the
    /// slot has been recycled by another `Delay`, we don't touch it.
    ///
    /// INVARIANT: a `Delay` is always polled on a single core (its executor's
    /// core) for its whole lifetime — no task migration in the cooperative model
    /// — so `me.slot`'s `(idx, gen)` always refers to that core's
    /// `PER_CORE_DELAYS` list. Both `free_slot` and the poll path use
    /// `my_list()` which routes to the same list via `cpu_id()`.
    fn free_slot(&mut self) {
        if let Some((idx, my_gen)) = self.slot.take() {
            without_interrupts(|| {
                let mut list = my_list().lock();
                if let Some(entry) = &list[idx] {
                    if entry.gen == my_gen {
                        list[idx] = None;
                    }
                }
            });
        }
    }
}

impl Future for Delay {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        // Delay has only Unpin fields (u64, Option<(usize, u64)>), so it
        // auto-implements Unpin and `get_mut` is safe.
        let me = self.get_mut();

        if crate::timer::ticks() >= me.target {
            me.free_slot();
            return Poll::Ready(());
        }

        without_interrupts(|| {
            let mut list = my_list().lock();
            // Already registered: refresh the waker IF our generation
            // still owns the slot. If the ISR consumed our slot between
            // polls (target reached on a borderline tick), our entry is
            // gone — fall through to find a new slot.
            if let Some((idx, my_gen)) = me.slot {
                let same_gen = matches!(&list[idx], Some(entry) if entry.gen == my_gen);
                if same_gen {
                    list[idx] = Some(Slot {
                        target: me.target,
                        waker: cx.waker().clone(),
                        gen: my_gen,
                    });
                    return Poll::Pending;
                }
                // Our slot was taken; release the stale (idx, gen).
                me.slot = None;
            }
            // Find a free slot and tag it with a fresh generation.
            for (i, s) in list.iter_mut().enumerate() {
                if s.is_none() {
                    let g = GEN_COUNTER.fetch_add(1, Ordering::Relaxed);
                    *s = Some(Slot {
                        target: me.target,
                        waker: cx.waker().clone(),
                        gen: g,
                    });
                    me.slot = Some((i, g));
                    return Poll::Pending;
                }
            }
            // All slots busy: degrade instead of panicking (a panic here halts
            // the whole kernel — that was the "system frozen" bug). Resolve the
            // delay immediately; the awaiting task proceeds a little early and
            // re-arms on its next loop. With 64 slots this is essentially never
            // reached, but it must never be fatal.
            Poll::Ready(())
        })
    }
}

impl Drop for Delay {
    fn drop(&mut self) {
        self.free_slot();
    }
}

/// Called from the timer ISR (`timer::timer_handler`) on every fire.
///
/// Drains `PER_CORE_DELAYS[cpu]` -- the slot list belonging to `cpu`, which MUST
/// be the id of the core whose timer fired. Uses `try_lock` so a contended list
/// never deadlocks the ISR; missed slots are picked up on the next tick (max
/// 10 ms delay at 100 Hz -- accepted trade-off, spec inv. 6).
///
/// Only the firing core ever calls `timer_tick_core` for its own list (the ISR
/// is per-core), so this is the sole ISR writer for `PER_CORE_DELAYS[cpu]`
/// (single-writer-per-slot invariant, spec inv. 5).
pub fn timer_tick_core(now: u64, cpu: u32) {
    if let Some(mut list) = PER_CORE_DELAYS[cpu as usize].0.try_lock() {
        for s in list.iter_mut() {
            // Match on a borrow first, then mutate, so we can pull the
            // waker out without leaving the slot in a half-empty state.
            let due = matches!(s, Some(entry) if entry.target <= now);
            if due {
                if let Some(entry) = s.take() {
                    entry.waker.wake();
                }
            }
        }
    }
}
