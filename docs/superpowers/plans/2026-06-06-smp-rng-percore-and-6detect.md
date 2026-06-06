# RNG per-core + Supervisor 6-detect Implementation Plan

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development / executing-plans. `- [ ]`.

**Goal:** Two low-risk, independent foundational items (bundled): (1) **RNG per-core** —
each core gets its own ChaCha20 CSPRNG (no cross-core lock); (2) **Supervisor 6-detect**
— per-core heartbeat + a BSP supervisor task that detects mute cores (detection only;
recovery = later 6-recover). Spec §11 + §2.3. Builds on the committed foundation
(1a/1b/Step2/3a/3b/3c/Step5).

**Why low-risk:** RNG per-core is a partition of an init-once global (no protocol). The
supervisor is read-only (reads atomics) + one async task on the BSP — adds NO cross-core
locks. Both give value now (RNG: zero cross-core contention on `random_get`/SSH keygen;
supervisor: liveness visibility — meaningful now that Step 5 freed the BSP executor).

**Prerequisites (committed):** fast cpu_id (1a — `cpu_id()` is cheap), per-core executor
(3b — `run_core`), AP timers (3a — every core is woken ~100 Hz so even idle cores bump
their heartbeat), Delay per-core (3a — supervisor uses `Delay`), Step 5 (BSP executor is
free → the supervisor async task actually runs).

**CHANGELOG:** next free on this branch (`ls CHANGELOG | grep -oE '^[0-9]+' | sort -n |
tail -1` → +1). Trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## PART 1 — RNG per-core

**Files:** `kernel/src/rng.rs`

Current: `static RNG: Mutex<Option<ChaCha20Rng>>` seeded once from RDRAND; `fill`/
`next_u64` lock it; `init()` idempotent.

- [ ] **Step 1: per-core RNG array** — Replace the single static with:
```rust
use crate::cpu::MAX_CPUS;
struct RngSlot(Mutex<Option<ChaCha20Rng>>);
static RNG: [RngSlot; MAX_CPUS] = { const S: RngSlot = RngSlot(Mutex::new(None)); [S; MAX_CPUS] };
static SEEDED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
```

- [ ] **Step 2: seed ALL slots on the BSP** — `init()` seeds every core's slot with a
  FRESH RDRAND draw (distinct streams; the BSP does it once before APs use RNG):
```rust
pub fn init() {
    use core::sync::atomic::Ordering;
    if SEEDED.load(Ordering::SeqCst) { return; }      // idempotent (boot-checks may call early)
    if !has_rdrand() { panic!("rng: CPU lacks RDRAND — no secure entropy source"); }
    for slot in RNG.iter() {
        let mut seed = [0u8; 32];
        for chunk in seed.chunks_mut(8) { chunk.copy_from_slice(&rdrand_u64().to_le_bytes()); }
        *slot.0.lock() = Some(ChaCha20Rng::from_seed(seed));
        for b in seed.iter_mut() { unsafe { core::ptr::write_volatile(b, 0) }; } // scrub
    }
    SEEDED.store(true, Ordering::SeqCst);
    crate::binfo!("rng", "chacha20 seeded per-core (rdrand) cores={}", MAX_CPUS);
}
```

- [ ] **Step 3: per-core fill/next_u64** — index by `cpu_id()`:
```rust
pub fn fill(buf: &mut [u8]) {
    RNG[crate::cpu::cpu_id() as usize].0.lock().as_mut().expect("rng: not initialized").fill_bytes(buf);
}
pub fn next_u64() -> u64 {
    RNG[crate::cpu::cpu_id() as usize].0.lock().as_mut().expect("rng: not initialized").next_u64()
}
```
> Each core touches ONLY `RNG[cpu_id()]` → the spin::Mutex is uncontended cross-core
> (kept for the rare same-core re-entrancy; RNG is not used from ISRs). All MAX_CPUS slots
> are seeded at init, so any AP that later calls fill/next_u64 finds its slot ready.

- [ ] **Step 4: boot-check — distinct per-core streams** — In `boot/phases/interrupts.rs`
  under boot-checks (after bringup), prove two cores draw different values via the bus or
  a pool job. Simplest: a pool job that returns `rng::next_u64()` (runs on some AP) vs the
  BSP's `rng::next_u64()` — they must differ (distinct seeds). OR just log the BSP's draw +
  one AP's draw via `inbox::request(1, |_| rng::next_u64() as ... )`. Concretely, reuse the
  message bus:
```rust
    #[cfg(feature = "boot-checks")]
    if crate::cpu::cpus_online() >= 2 {
        fn draw(_in: &[u8]) -> u64 { crate::rng::next_u64() }
        let bsp = crate::rng::next_u64();
        // run draw() on core 1 via the inbox, drive inline (no executor here)
        let mut fut = crate::smp::inbox::request(1, draw, alloc::boxed::Box::from(&[][..]));
        use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        fn noop(_: *const ()) {} fn cl(_: *const ()) -> RawWaker { RawWaker::new(core::ptr::null(), &VT) }
        static VT: RawWakerVTable = RawWakerVTable::new(cl, noop, noop, noop);
        let w = unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VT)) };
        let mut cx = Context::from_waker(&w);
        let mut ap = None;
        for _ in 0..50_000_000u64 { if let Poll::Ready(v) = core::future::Future::poll(core::pin::Pin::new(&mut fut), &mut cx) { ap = Some(v); break; } core::hint::spin_loop(); }
        crate::binfo!("rng", "percore distinct bsp!=ap1 -> {}", ap.map_or(false, |a| a != bsp));
    }
```
Expect `rng percore distinct ... -> true`. (`draw` is a `fn(&[u8])->u64` = inbox op; it
runs on core 1 → `rng::next_u64()` there uses RNG[1] ≠ RNG[0].)

- [ ] **Step 5: build + commit** — `make test-boot` → `TEST_BOOT_PASS` (single core: RNG
  seeded, fill/next_u64 use RNG[0]; existing SSH keygen + random_get still work). Then the
  -smp 4 distinct check (Step 4). Commit:
```
git add kernel/src/rng.rs kernel/src/boot/phases/interrupts.rs
git commit -m "feat(smp): per-core RNG (ChaCha20 per core, distinct seeds, no cross-core lock)"
```
Trailer as above. (interrupts.rs also gets the 6-detect bits below — you may stage it once
with both, or commit RNG first then 6-detect; either is fine.)

---

## PART 2 — Supervisor 6-detect (per-core heartbeat + mute detection)

**Files:** `kernel/src/sched/cpustat.rs` (or a new `sched/heartbeat.rs`),
`kernel/src/executor/mod.rs`, `kernel/src/cpu/ap.rs`/`wm.rs` (bump sites),
`kernel/src/boot/phases/interrupts.rs`.

- [ ] **Step 1: heartbeat array** — In `sched/cpustat.rs` (co-located with per-core sched
  state) add:
```rust
use core::sync::atomic::{AtomicU64, Ordering};
static HEARTBEAT: [AtomicU64; crate::cpu::MAX_CPUS] = { const Z: AtomicU64 = AtomicU64::new(0); [Z; crate::cpu::MAX_CPUS] };
#[inline] pub fn heartbeat_bump(cpu: usize) { HEARTBEAT[cpu].fetch_add(1, Ordering::Relaxed); }
pub fn heartbeat(cpu: usize) -> u64 { HEARTBEAT[cpu].load(Ordering::Relaxed) }
```

- [ ] **Step 2: bump in EVERY core's main loop** — A core idle in `hlt` is woken ~100 Hz
  by its LAPIC timer (3a), loops, and bumps — so idle cores still advance; only a TRULY
  stuck core (non-yielding loop) stops bumping. Add `crate::sched::cpustat::heartbeat_bump(cpu)`:
  - in `executor::run_core`'s loop (covers BSP + ComputeApp APs) — once per iteration.
  - in `wm.rs run_compositor_gate`'s frame loop (covers the GUI core) — once per frame.
  - in `wm.rs gui_worker_loop`'s wait loop (covers the GUI core BEFORE the compositor
    arrives — it's woken by the timer, bumps, re-halts).
  (Read each loop; add the single bump line. `cpu` = `crate::cpu::cpu_id()` where not
  already in scope.)

- [ ] **Step 3: supervisor task** — In `executor/mod.rs` add an async task spawned on the
  BSP (in `run_core` when `cpu == 0`):
```rust
#[embassy_executor::task]
async fn supervisor_task() {
    use core::sync::atomic::Ordering;
    // Snapshot, wait ~1s, compare. A core whose heartbeat didn't advance over the window
    // AND is supposed to be running is "mute". Detection only (6-recover kills later).
    let mut prev = [0u64; crate::cpu::MAX_CPUS];
    let mut first = true;
    loop {
        let n = crate::cpu::cpus_online() as usize;
        crate::executor::delay::Delay::ticks(100).await; // ~1 s
        let mut alive = 0u32; let mut mute = 0u32;
        for c in 0..n {
            let h = crate::sched::cpustat::heartbeat(c);
            if h != prev[c] { alive += 1; } else if !first { mute += 1; }
            prev[c] = h;
        }
        if first {
            crate::binfo!("super", "supervisor up, watching {} cores", n);
            first = false;
        } else if mute > 0 {
            crate::bwarn!("super", "mute cores={} alive={}/{}", mute, alive, n);
        } else {
            crate::binfo!("super", "all {} cores alive", n);   // greppable liveness marker
        }
    }
}
```
Spawn it in `run_core`'s `if cpu == 0 { ... }` block: `spawner.spawn(supervisor_task()).unwrap();`.
> 6-detect = DETECTION/logging only. NO recovery (no killing) — that's 6-recover, which
> needs the per-core process registries (Step 4). Do not add kill logic here.

- [ ] **Step 4: gate (boot-check / serial marker)** — The supervisor logs `all N cores
  alive` each second once heartbeats advance. Build + boot `-smp 4`, observe over a few
  seconds:
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso CARGO_FEATURES="boot-checks" && timeout 90 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -m 512 -no-reboot -display none -serial stdio -device qemu-xhci -cdrom build/os.iso 2>&1 | grep -E "supervisor up|cores alive|mute cores|rng percore|APs online" | head -10'
```
GATE: `supervisor up, watching 4 cores` then `all 4 cores alive` (repeated) AND `rng
percore distinct ... -> true`. **`mute cores=` must NOT appear** (no false positives —
the GUI core idle-waiting still bumps via its timer wake; all 4 cores advance). If a
core shows mute while genuinely alive, the bump is missing on that core's loop (check the
GUI-core / gui_worker_loop bump) → fix. Also `make test-boot` (1 core) → `TEST_BOOT_PASS`
(supervisor logs `all 1 cores alive`).

- [ ] **Step 5: regression** — `make run-smp-test` + `make run-smp2-test` (the run_core
  loops gained a heartbeat bump — negligible; confirm still green). `make run-ssh-gui-test`
  (the GUI/compositor loops gained a bump — confirm the goal still holds + the GUI core
  shows alive while the compositor runs).

- [ ] **Step 6: changelog + commit** —
```
git add kernel/src/sched/cpustat.rs kernel/src/executor/mod.rs kernel/src/cpu/ap.rs kernel/src/wasm/wt/wm.rs kernel/src/boot/phases/interrupts.rs CHANGELOG/NN-...
git commit -m "feat(smp): supervisor 6-detect — per-core heartbeat + mute-core detection (no recovery yet)"
```
Trailer as above.

---

## Self-Review
- **RNG per-core:** all MAX_CPUS slots seeded once on the BSP from fresh RDRAND → distinct
  streams, each core touches only its slot (no cross-core lock). Idempotent via `SEEDED`.
  Single-core unchanged (uses RNG[0]). Gate: `bsp != ap1` draw.
- **Heartbeat avoids false-mute:** every core bumps in its main loop; idle cores are woken
  ~100 Hz by their LAPIC timer (3a) → still advance. Only a truly-stuck (non-yielding,
  IF-disabled spin / hang) core stops bumping → detected. The GUI core bumps in BOTH
  gui_worker_loop (pre-compositor) and run_compositor_gate (post) → never false-mute.
- **Supervisor is read-only + BSP-async:** reads atomics, no new cross-core lock. It runs
  because Step 5 freed the BSP executor (pre-Step-5 the compositor would have starved it —
  the spec §13 cross-step risk, now resolved). DETECTION ONLY; recovery is 6-recover.
- **No recovery here:** killing a mute core's WASM instances needs per-core process
  registries (Step 4). 6-detect logs; 6-recover acts. Keep them separate.
- **Risk: LOW.** Worst case a missing bump on one core's loop → false `mute` log (cosmetic,
  caught by the gate). RNG is a clean partition. Neither adds cross-core locks.