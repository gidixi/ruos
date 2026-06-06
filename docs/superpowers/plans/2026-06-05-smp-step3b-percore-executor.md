# Step 3b — Per-core executor Implementation Plan

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development / executing-plans. `- [ ]`.

**Goal:** Each core runs its OWN cooperative executor (its run-queue + Delay list +
idle/hlt). APs enter `executor::run_core(cpu)` instead of `ap_worker_loop`; the compute
pool drain moves INTO the per-core loop so banded compositing keeps its workers. The BSP
becomes `run_core(0)` (spawning the existing I/O task set). Spec §8; follows 3a (per-core
Delay + AP timer) which is committed + verified.

**Prerequisites (committed):** 1a (fast cpu_id), 1b (magazine), Step 2 (per-core
`WAKE_PENDING` + per-core `__pender` + message bus + targeted IPI), 3a (per-core Delay
lists + AP LAPIC timer at 100 Hz — verified `ap1 ticks ≈ 5/50ms`).

**Key facts (from code):**
- `executor::run()` (mod.rs:51): builds the singleton `EXECUTOR` (ExecCell), spawns 11
  I/O tasks, loops {clear WAKE_PENDING[0], poll, drain_inbox(0), halt gated on
  WAKE_PENDING[0]||is_pending(0)}. (Step 2 already made WAKE_PENDING per-core + inbox-aware.)
- `__pender(context)` (Step 2): `wake_core(context as u32)` — already per-core. So an
  executor created with context = cpu id wakes the right core.
- `cpu/ap.rs ap_entry`: ...→ `start_ap_timer()` (3a) → `mark_online()` → `ap_worker_loop()`
  (drains `smp::pool` + hlt). This `ap_worker_loop` is what 3b REPLACES with `run_core`.
- `smp::pool::{take, run_slot, is_empty}` — the compute pool the APs drain today.

**Invariants:** per-core executor is single-writer (each core polls ONLY its own
`PER_CORE_EXECUTOR[cpu]`; no cross-core executor access — cross-core task injection is
3c, via queue + IPI). `RawExecutor::poll` called serially per core. No missed wakes
(halt under IF-disable, gated on wake||pool||inbox).

---

## File Structure
- `kernel/src/executor/mod.rs` — `PER_CORE_EXECUTOR[MAX_CPUS]`; `run_core(cpu)` (unified
  loop: poll + drain inbox + drain pool, halt-gated); `run()` → thin wrapper calling
  `run_core(0)` (BSP spawns the I/O tasks); update the ExecCell Sync doc.
- `kernel/src/cpu/ap.rs` — `ap_entry` calls `executor::run_core(cpu_id)` instead of
  `ap_worker_loop`. Keep `ap_worker_loop` deleted or `#[allow(dead_code)]`-retained? →
  DELETE it (its pool-drain logic moves into `run_core`).
- `kernel/src/boot/phases/interrupts.rs` — 3b boot-check: AP1 heartbeat counter grows.
- `CHANGELOG/NN`.

---

## Task 1: `PER_CORE_EXECUTOR` + `run_core(cpu)`

**Files:** `kernel/src/executor/mod.rs`

- [ ] **Step 1: per-core executor array** — Replace the singleton `EXECUTOR` with:
```rust
struct ExecCell(UnsafeCell<MaybeUninit<RawExecutor>>);
// SAFETY: each core touches ONLY its own PER_CORE_EXECUTOR[cpu_id] slot — it is the
// sole writer (in run_core) and sole caller of its own exec.poll(). No core ever polls
// or spawns into another core's executor; cross-core task injection (Step 3c) goes
// through a per-core spawn queue + IPI, never a direct touch of a remote RawExecutor.
unsafe impl Sync for ExecCell {}
static PER_CORE_EXECUTOR: [ExecCell; crate::cpu::MAX_CPUS] = {
    const E: ExecCell = ExecCell(UnsafeCell::new(MaybeUninit::uninit()));
    [E; crate::cpu::MAX_CPUS]
};
```

- [ ] **Step 2: `run_core(cpu)`** — The unified per-core loop. The BSP (cpu 0) spawns the
  existing I/O task set; APs spawn nothing here (3c injects later; the 3b test injects an
  AP-local heartbeat under boot-checks):
```rust
/// Run this core's cooperative executor forever. `cpu` is the dense core id; it is
/// encoded into the executor context so `__pender` (Step 2) wakes THIS core.
pub fn run_core(cpu: u32) -> ! {
    // SAFETY: called exactly once per core, on that core. Sole writer of this slot.
    let exec: &'static RawExecutor = unsafe {
        let slot = &mut *PER_CORE_EXECUTOR[cpu as usize].0.get();
        slot.write(RawExecutor::new(cpu as usize as *mut ()))   // context = owner core id
    };
    let spawner = exec.spawner();
    if cpu == 0 {
        // BSP owns the I/O task set (unchanged from the old run()).
        spawner.spawn(tick_task()).unwrap();
        spawner.spawn(net_poll_task()).unwrap();
        spawner.spawn(usb_poll_task()).unwrap();
        spawner.spawn(console_drain_task()).unwrap();
        spawner.spawn(boot_shell_task()).unwrap();
        spawner.spawn(exec_worker_task()).unwrap();
        spawner.spawn(pipeline_worker_task()).unwrap();
        spawner.spawn(ssh_serve_task()).unwrap();
        spawner.spawn(ssh_pty_dispatcher_task()).unwrap();
        spawner.spawn(pty_watchdog_task()).unwrap();
        spawner.spawn(service_dispatcher_task()).unwrap();
        crate::binfo!("user", "executor: core 0 tasks spawned");
    }
    // 3b test hook: AP 1 runs a heartbeat task (proves per-core executor + AP Delay +
    // AP timer end-to-end). Only under boot-checks.
    #[cfg(feature = "boot-checks")]
    if cpu == 1 {
        spawner.spawn(heartbeat_task()).unwrap();
    }
    loop {
        WAKE_PENDING[cpu as usize].store(false, Ordering::SeqCst);
        let poll_start = crate::boot::clock::read_tsc();
        unsafe { exec.poll(); }
        crate::smp::inbox::drain_inbox(cpu);
        // Drain the compute pool so banded compositing keeps workers (moved here from
        // the old ap_worker_loop). Any core may take pool jobs.
        while let Some(slot) = crate::smp::pool::take() {
            crate::smp::pool::run_slot(slot, cpu);
        }
        crate::sched::cpustat::add_busy(cpu as usize, crate::boot::clock::read_tsc().saturating_sub(poll_start));

        interrupts::disable();
        let more = WAKE_PENDING[cpu as usize].load(Ordering::SeqCst)
            || crate::smp::inbox::is_pending(cpu)
            || !crate::smp::pool::is_empty();
        if more {
            interrupts::enable();
        } else {
            let hlt_start = crate::boot::clock::read_tsc();
            interrupts::enable_and_hlt();
            crate::sched::cpustat::add_idle(cpu as usize, crate::boot::clock::read_tsc().saturating_sub(hlt_start));
        }
    }
}
```

- [ ] **Step 3: `run()` → `run_core(0)`** — Replace the old `run()` body with:
```rust
/// BSP entry (kept for call-site compatibility). Drives core 0's executor.
pub fn run() -> ! { run_core(0) }
```
Delete the old singleton `EXECUTOR` static + the old loop body (now in `run_core`).

- [ ] **Step 4: `heartbeat_task` (boot-checks only)** — Add near the other tasks:
```rust
#[cfg(feature = "boot-checks")]
pub static HEARTBEAT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

#[cfg(feature = "boot-checks")]
#[embassy_executor::task]
async fn heartbeat_task() {
    loop {
        HEARTBEAT.fetch_add(1, Ordering::SeqCst);
        crate::executor::delay::Delay::ticks(2).await;   // ~20 ms; uses THIS core's Delay
    }
}
```
(Confirm the `#[embassy_executor::task]` macro + the task arena allow one more task.)

- [ ] **Step 5: build** — `make test-boot` (1 core: BSP runs run_core(0) with the I/O
  tasks; no AP). Expected `TEST_BOOT_PASS` — proves the BSP executor refactor is behavior-
  preserving (all I/O tasks still run: shell prompt, etc.). The cpu==1 heartbeat is not
  spawned (1 core). If it fails, the run_core refactor broke the BSP path — fix before
  continuing.

- [ ] **Step 6: commit** —
```
git add kernel/src/executor/mod.rs
git commit -m "feat(smp): 3b — per-core executor (run_core); run()=run_core(0) (Step 3b part 1)"
```
Trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## Task 2: APs enter `run_core` (retire `ap_worker_loop`)

**Files:** `kernel/src/cpu/ap.rs`

- [ ] **Step 1: ap_entry → run_core** — In `ap_entry`, replace the final
  `ap_worker_loop()` call with `crate::executor::run_core(cpu_id as u32)`. Keep the
  ordering: gdt/idt/lapic init → `set_tsc_aux` (1a) → `start_ap_timer` (3a) →
  `mark_online()` → `run_core(cpu_id)`. The AP now runs a real per-core executor (its
  loop drains the compute pool too, so the SMP compositing pool still works).

- [ ] **Step 2: delete `ap_worker_loop`** — Its job (drain pool + hlt) is now in
  `run_core`. Remove the function (or the AP-only pool loop) to avoid dead code. Keep the
  module doc accurate (update the "enter a compute WORKER loop" wording → "enter its
  per-core cooperative executor").

- [ ] **Step 3: build** — `make test-boot` (1 core, no AP) → `TEST_BOOT_PASS`. Then
  `make iso CARGO_FEATURES="boot-checks"` + boot `-smp 4`:
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 90 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -m 512 -no-reboot -display none -serial stdio -device qemu-xhci -cdrom build/os.iso 2>&1 | grep -E "APs online|executor|core 0 tasks"'
```
Expect `3/3 APs online` (APs now in run_core, still register online). The system must
still boot to a shell.

- [ ] **Step 4: regression — the compute pool still works via run_core** —
  `make run-smp-test` + `make run-smp2-test` (these exercise `smp::pool` — the banded/
  parallel compute path the APs now drain inside run_core). Both must PASS with the AP
  speedup intact (smp2 reported ~2.7x earlier). Also `make run-comp-smp-test` if it runs
  (SP4 banded compositing equivalence) — confirms compositing workers survived the move.

- [ ] **Step 5: commit** —
```
git add kernel/src/cpu/ap.rs
git commit -m "feat(smp): 3b — APs run per-core executor (run_core), retire ap_worker_loop (Step 3b part 2)"
```
Trailer as above.

---

## Task 3: 3b gate — AP1 heartbeat boot-check

**Files:** `kernel/src/boot/phases/interrupts.rs`, `CHANGELOG/NN`

- [ ] **Step 1: boot-check** — In `boot/phases/interrupts.rs` `#[cfg(feature="boot-checks")]`
  after bringup (near the 3a timer check), confirm AP1's executor is actually running its
  heartbeat task (which uses AP1's Delay + timer):
```rust
    #[cfg(feature = "boot-checks")]
    if crate::cpu::cpus_online() >= 2 {
        let h0 = crate::executor::HEARTBEAT.load(core::sync::atomic::Ordering::SeqCst);
        let start = crate::timer::ticks();
        while crate::timer::ticks() < start + 10 { core::hint::spin_loop(); } // ~100 ms
        let grew = crate::executor::HEARTBEAT.load(core::sync::atomic::Ordering::SeqCst).saturating_sub(h0);
        crate::binfo!("exec", "ap1 heartbeat ticks in 100ms = {} (expect ~5)", grew);
    }
```
(Heartbeat bumps every `Delay::ticks(2)` ≈ 20 ms → ~5 in 100 ms. This proves AP1's
per-core EXECUTOR polls, its Delay registers in AP1's per-core list, and AP1's timer
wakes it — the full 3a+3b chain. `grew == 0` ⇒ AP1's executor isn't polling or its Delay/
timer is broken.)

- [ ] **Step 2: build + run -smp 4 (the gate)** —
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso CARGO_FEATURES="boot-checks" && for i in 1 2; do echo "run $i"; timeout 90 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -m 512 -no-reboot -display none -serial stdio -device qemu-xhci -cdrom build/os.iso 2>&1 | grep -E "ap1 heartbeat|ap1 ticks|APs online"; done'
```
GATE: `ap1 heartbeat ticks in 100ms = N` with **N > 0 and roughly 5** (and stable across
both runs). N==0 ⇒ AP1's executor never polled the heartbeat (run_core broken on the AP)
OR its Delay never fired (per-core Delay / AP timer broken). Do NOT mark 3b done on N==0.
Also `make test-boot` (1 core) → `TEST_BOOT_PASS` + the check skipped (1 core).

- [ ] **Step 3: changelog + commit** — next free number. Cosa: 3b — per-core executor;
  APs run run_core; pool drain folded in; AP1 heartbeat boot-check proves per-core
  executor + Delay + timer end-to-end. Perché: foundation for cross-core spawn (3c) and
  pinning workloads to cores (Step 5).
```
git add kernel/src/boot/phases/interrupts.rs CHANGELOG/NN-...
git commit -m "test(smp): 3b — AP1 heartbeat boot-check (per-core executor + Delay end-to-end)"
```
Trailer as above.

---

## Self-Review
- **BSP behavior-preserving:** `run()` = `run_core(0)` spawns the SAME 11 tasks + the same
  loop shape (clear WAKE_PENDING[0], poll, drain inbox, halt-gated) PLUS a pool drain
  (the BSP can now also drain pool jobs — harmless fallback, the BSP already could).
  test-boot (1 core) is the gate that the I/O path didn't regress.
- **Pool workers preserved:** the AP pool drain moved from `ap_worker_loop` into
  `run_core`'s loop, so `smp::pool` still has drainers (run-smp-test/comp-smp-test gate).
- **Per-core executor single-writer:** each core writes/polls only `PER_CORE_EXECUTOR[cpu]`.
  Cross-core spawn is 3c (queue+IPI), NOT a direct remote poke — the Sync doc says so.
- **Missed-wake:** halt gated on WAKE_PENDING[cpu]||inbox||pool under IF-disable + sti;hlt.
- **AP1 heartbeat uses AP1's Delay+timer** (3a) — the gate proves the whole chain.
- **Risk:** APs enter run_core during bringup (interrupts phase) while the BSP continues
  boot (fs/storage/usb) and only calls run_core(0) in the userland phase — the APs run
  their (near-empty) executors meanwhile, which is fine (poll empty queue + drain pool/
  inbox + halt). If an AP's run_core touches something not yet initialised at bringup
  time, that's a sequencing bug — the heartbeat is gated to cpu==1 + boot-checks and uses
  only Delay (timer + per-core list, both up by ap_entry). Watch for it.
