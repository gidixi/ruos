# Step 3 — Executor per-core (decomposed) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or executing-plans. `- [ ]` checkboxes.

**Goal:** Each core runs its own cooperative executor (its run-queue + Delay list +
idle/hlt), woken by its own LAPIC timer and by the cross-core wake primitive (Step 2).
Spec: `2026-06-05-smp-shared-nothing-migration-design.md` §8. This is the largest, most
concurrency-critical step — **decomposed into 4 sub-steps, each its own milestone**:

- **3a — AP timer + per-core Delay lists** (THIS plan, fully detailed below). The
  prerequisite: an AP gets its own 100 Hz tick and drains its own Delay list.
- **3b — per-core executor** (outlined; own plan when reached). APs enter
  `executor::run_core(id)` and run real tasks; pool drain folded in.
- **3c — cross-core spawn** (outlined). `spawn_on(cpu, task)` — exercises Step 2 wake.
- **3d — TLB shootdown** (outlined). `VEC_TLB_SHOOTDOWN` for shared-MAPPER mutations.

**Prerequisites (committed):** Step 1a (fast cpu_id), Step 1b (magazine), Step 2
(per-core `WAKE_PENDING`, per-core `__pender`, message bus, targeted IPI, `VEC_WAKE`/
`VEC_INBOX`/`VEC_TLB_SHOOTDOWN`(reserved)).

**Invariants to preserve (spec §2):** `TICKS` stays ONE global counter (only the BSP
increments it — invariant 8); per-core state is single-writer-per-slot (invariant 5);
ISR paths stay lock-light (`try_lock`-defer, invariant 6); no missed wakes (invariant 7).

---

## Sub-step 3a — AP timer + per-core Delay lists

**Why first:** an AP executor (3b) can only wake `Delay`-using tasks if that AP has a
periodic timer that drains the AP's own Delay list. Today only the BSP times, and the
Delay list is one global list (`SLOTS_LIST`) drained by the shared `timer_handler`.

### File Structure
- `kernel/src/executor/delay.rs` — `SLOTS_LIST` → `PER_CORE_DELAYS[MAX_CPUS]`;
  `Delay::poll`/`free_slot` index by `cpu_id()`; `timer_tick(now)` → `timer_tick_core(now, cpu)`.
- `kernel/src/timer.rs` — `timer_handler` per-core-aware (BSP-only TICKS + cursor; every
  core drains its own Delay list); publish the calibrated periodic count; add
  `start_ap_timer()`.
- `kernel/src/apic/lapic.rs` — `init_ap` no longer hard-masks the timer (or add an
  explicit unmask); confirm `set_timer_periodic` is callable on an AP.
- `kernel/src/cpu/ap.rs` — after `lapic::init_ap`, call `timer::start_ap_timer()` to arm
  this AP's periodic timer.
- `kernel/src/boot/phases/interrupts.rs` — boot-check: confirm an AP's tick advances.
- `CHANGELOG/NN`.

### Task 1: Per-core Delay lists

**Files:** `kernel/src/executor/delay.rs`

- [ ] **Step 1: per-core SLOTS_LIST** — Replace the single
  `static SLOTS_LIST: Mutex<[Option<Slot>; SLOTS]>` with a per-core array:
```rust
struct DelayList(Mutex<[Option<Slot>; SLOTS]>);
impl DelayList { const fn new() -> Self { Self(Mutex::new([NONE_SLOT; SLOTS])) } }
static PER_CORE_DELAYS: [DelayList; crate::cpu::MAX_CPUS] = {
    const L: DelayList = DelayList::new();
    [L; crate::cpu::MAX_CPUS]
};
#[inline] fn my_list() -> &'static Mutex<[Option<Slot>; SLOTS]> {
    &PER_CORE_DELAYS[crate::cpu::cpu_id() as usize].0
}
```
`GEN_COUNTER` stays a single global `AtomicU64` (it only needs to be unique; multi-core
`fetch_add` is fine — the gen tag is matched within one list, but global uniqueness is
harmless and simplest).

- [ ] **Step 2: index the task-side accesses by core** — In `Delay::free_slot` and
  `Delay::poll`, replace `SLOTS_LIST.lock()` with `my_list().lock()`. The future is
  polled on its owner core (cooperative, no migration), so registration + free happen on
  the same core's list. The slot index stored in `me.slot` is now an index into THIS
  core's list — that holds because the same task always polls on the same core.
  > INVARIANT (document in a comment): a `Delay` is always polled on a single core (its
  > executor's core) for its whole lifetime — no task migration in the cooperative model
  > — so `me.slot`'s `(idx, gen)` always refers to that core's `PER_CORE_DELAYS` list.

- [ ] **Step 3: `timer_tick_core`** — Rename `timer_tick(now)` →
  `timer_tick_core(now: u64, cpu: u32)`, draining `PER_CORE_DELAYS[cpu].0` (via
  `try_lock`, unchanged logic otherwise):
```rust
pub fn timer_tick_core(now: u64, cpu: u32) {
    if let Some(mut list) = PER_CORE_DELAYS[cpu as usize].0.try_lock() {
        for s in list.iter_mut() {
            let due = matches!(s, Some(entry) if entry.target <= now);
            if due { if let Some(entry) = s.take() { entry.waker.wake(); } }
        }
    }
}
```

- [ ] **Step 4: build** — `make test-boot` (BSP only uses `PER_CORE_DELAYS[0]` now).
  Expected `TEST_BOOT_PASS` — the BSP's Delay-using tasks (net poll, tick, etc.) still
  wake correctly. (timer.rs Task 2 wires the new name; do Task 2 before building.)

### Task 2: Per-core timer handler + AP timer arming

**Files:** `kernel/src/timer.rs`, `kernel/src/apic/lapic.rs`, `kernel/src/cpu/ap.rs`

- [ ] **Step 1: publish the calibrated count** — In `timer.rs`, store the periodic
  count so APs can program their timers with the same value:
```rust
static AP_TIMER_COUNT: AtomicU32 = AtomicU32::new(0);
```
In `init()`, after computing `initial_count`, add `AP_TIMER_COUNT.store(initial_count, Ordering::SeqCst);` (before `set_timer_periodic`).

- [ ] **Step 2: per-core `timer_handler`** — Rewrite so only the BSP advances the global
  clock + cursor; every core drains its OWN Delay list:
```rust
pub extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    let cpu = crate::cpu::cpu_id();
    let now = if cpu == 0 {
        let n = TICKS.fetch_add(1, Ordering::Relaxed) + 1; // BSP owns the wall clock
        crate::console::fb::tick_cursor();
        n
    } else {
        TICKS.load(Ordering::Relaxed) // APs read the shared clock, never increment it
    };
    crate::executor::delay::timer_tick_core(now, cpu);
    lapic::eoi();
}
```

- [ ] **Step 3: `start_ap_timer`** — In `timer.rs`, add an AP entry point that arms this
  core's LAPIC timer with the published count:
```rust
/// Arm THIS (AP) core's LAPIC timer in periodic mode with the count the BSP
/// calibrated in `init()`. No-op (returns) if calibration hasn't run yet.
pub fn start_ap_timer() {
    let count = AP_TIMER_COUNT.load(Ordering::SeqCst);
    if count == 0 { return; }
    lapic::set_timer_periodic(idt::VEC_LAPIC_TIMER, count);
}
```

- [ ] **Step 4: AP timer not masked** — In `apic/lapic.rs`, `init_ap` currently writes
  `TIMER_MASKED` to the timer LVT. `start_ap_timer`/`set_timer_periodic` reprograms the
  LVT with the vector (unmasked) — confirm `set_timer_periodic` writes the LVT with the
  vector (not masked) and a periodic mode bit. READ `set_timer_periodic` + `init_ap` and
  ensure the AP path ends UNMASKED after `start_ap_timer`. If `init_ap`'s mask races with
  a later `start_ap_timer`, that's fine (start_ap_timer runs after init_ap in ap_entry).

- [ ] **Step 5: arm the AP timer in ap_entry** — In `cpu/ap.rs ap_entry`, AFTER
  `crate::apic::lapic::init_ap(...)` and BEFORE `mark_online()`/the worker loop, add:
```rust
    crate::timer::start_ap_timer();
```
(The AP now receives 100 Hz timer IRQs → its `timer_handler` drains
`PER_CORE_DELAYS[cpu_id()]`. The AP worker loop's `enable_and_hlt` will be woken by these
ticks too — that is fine; it re-checks the pool/inbox and re-halts.)

- [ ] **Step 6: boot-check — AP tick advances** — In `boot/phases/interrupts.rs`
  `#[cfg(feature="boot-checks")]` after bringup, verify an AP is actually timing. Use a
  per-core tick counter incremented in `timer_handler` for APs, OR simplest: have an AP
  run a tiny job via the pool that records `TICKS`, sleep ~3 ticks (busy-wait on TICKS
  delta on the BSP), and confirm the AP saw the timer fire. Concretely add a
  `static AP_TICKS: [AtomicU64; MAX_CPUS]` in timer.rs bumped by `timer_handler` for cpu>0,
  and the boot-check reads `AP_TICKS[1]` before/after a ~50 ms BSP wait and asserts it
  grew:
```rust
    #[cfg(feature = "boot-checks")]
    if crate::cpu::cpus_online() >= 2 {
        let t0 = crate::timer::ap_ticks(1);
        let start = crate::timer::ticks();
        while crate::timer::ticks() < start + 5 { core::hint::spin_loop(); } // ~50 ms
        let grew = crate::timer::ap_ticks(1).saturating_sub(t0);
        crate::binfo!("timer", "ap1 ticks in 50ms = {} (expect > 0)", grew);
    }
```
(Add `AP_TICKS` + `pub fn ap_ticks(cpu: u32) -> u64` + the bump in `timer_handler` for
cpu>0.)

- [ ] **Step 7: build + run -smp 4** — `make iso CARGO_FEATURES="boot-checks"` then boot
  `-smp 4`; grep `timer`. Expect `ap1 ticks in 50ms = N` with N≈5 (a 100 Hz AP timer over
  ~50 ms). Also `make test-boot` (1 core) → `TEST_BOOT_PASS` (no AP, the check is skipped).
  Regression: `make run-smp-test` + `make run-smp2-test` (the AP loop now also gets timer
  IRQs — the pool must still work).

- [ ] **Step 8: changelog + commit** — next free number. Cosa: 3a — per-core Delay lists
  + per-core timer handler + AP LAPIC timer armed (BSP keeps the global TICKS). Perché:
  prerequisite for per-core executors (3b) — an AP must drain its own Delay list on its
  own tick.
```
git add kernel/src/executor/delay.rs kernel/src/timer.rs kernel/src/apic/lapic.rs kernel/src/cpu/ap.rs kernel/src/boot/phases/interrupts.rs CHANGELOG/NN-...
git commit -m "feat(smp): 3a — per-core Delay lists + AP LAPIC timer (BSP keeps global TICKS)"
```
Trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

### 3a Self-Review
- **TICKS single-writer:** only `cpu==0` calls `fetch_add` (invariant 8). APs read-only.
- **Per-core Delay single-writer:** each core's list is locked only by that core's task
  side (`my_list`) + that core's ISR (`timer_tick_core(_, cpu)` with cpu == this core).
  No cross-core writer to a list (invariant 5). The cursor tick stays BSP-only.
- **GEN_COUNTER multi-writer:** global `fetch_add` — atomic, fine; gen only needs
  per-list uniqueness, global is a superset.
- **ISR lock-light:** `timer_tick_core` keeps `try_lock`-defer (invariant 6).
- **Risk:** the AP timer firing wakes the AP from `hlt` every 10 ms even with no work —
  the worker loop re-checks pool+inbox and re-halts (tiny wakeup cost, acceptable; 3b's
  executor will use the ticks for Delay). If the AP timer calibration count is wrong, the
  AP either never ticks (count too large) or storms (count tiny) — Step 7's `ap1 ticks ≈ 5`
  gate catches both. Do NOT mark 3a done unless the AP tick count is sane (~5 in 50 ms).

---

## Sub-step 3b — Per-core executor (OUTLINE — detail in its own plan when reached)

`PER_CORE_EXECUTOR[MAX_CPUS]` (each an `ExecCell`); `executor::run_core(cpu)` builds the
core's executor with context = cpu id (so `__pender` wakes the right core, Step 2), then
loops: clear `WAKE_PENDING[cpu]`, poll, drain inbox(cpu), **drain the compute pool**
(so banded compositing still has workers), halt gated on `WAKE_PENDING[cpu] || pool work
|| inbox pending`. APs: `ap_entry` calls `run_core(cpu)` instead of `ap_worker_loop`
(the pool drain moves INTO run_core). The `ExecCell` Sync assertion is updated (each core
owns its own executor; no cross-core executor access — only cross-core ENQUEUE via 3c).
Test: spawn a heartbeat task on AP 1 that `Delay::ticks(10).await` loops + bumps a
counter; boot-check confirms the counter grows (proves per-core executor + per-core Delay
+ AP timer end-to-end). Risk: the `RawExecutor` run-queue is not cross-core-safe — each
core polls ONLY its own; cross-core task injection is 3c (queue + IPI), never a direct
`spawn` on another core's executor.

## Sub-step 3c — Cross-core spawn (OUTLINE)

`PER_CORE_SPAWN_QUEUE[MAX_CPUS]` (`IrqMutex<VecDeque<spawn-request>>`); `spawn_on(cpu,
fut)` enqueues + `executor::wake_core(cpu)` (Step 2). The target core's `run_core` drains
the spawn queue before polling, spawning each onto its LOCAL executor via its local
`Spawner`. This is where the Step-2 cross-core `__pender`/`wake_core` IPI path is finally
exercised end-to-end. Test: BSP `spawn_on(1, task)`; the task runs on core 1 (marker
tagged with `cpu_id()==1`).

## Sub-step 3d — TLB shootdown (OUTLINE — correctness gap, spec §13.1)

Once APs run executors that touch WASM/guest memory and the shared `MAPPER` (one PML4)
flips W^X or unmaps (module load/teardown, DMA), a core caching a now-stale PTE uses a
stale translation = silent memory-safety hole. Add `VEC_TLB_SHOOTDOWN` (0x42, reserved in
Step 2): the core mutating a mapping, while holding `MAPPER`, broadcasts a shootdown IPI
to the other online cores; each handler does `invlpg`(range) / full `mov cr3,cr3` and
ACKs via an atomic countdown; the initiator waits for all ACKs before releasing `MAPPER`
(never across an await). Test: map a page, read it on core A, unmap it on the BSP with a
shootdown, confirm core A faults on the next access (a trap page) — i.e. no stale TLB.
This can land after 3b/3c but BEFORE any AP executor mutates shared mappings under load.

---

## Overall Step 3 sequencing
`3a (AP timer + per-core Delay)` → `3b (per-core executor)` → `3c (cross-core spawn)` →
`3d (TLB shootdown)`. Each is its own commit(s) + boot-check gate. 3a is fully specified
above; 3b/3c/3d get detailed plans when reached. Do NOT batch them — each touches
concurrency invariants and needs its own `-smp 4` gate.
