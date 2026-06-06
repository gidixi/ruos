# Step 3c — Cross-core spawn Implementation Plan

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development / executing-plans. `- [ ]`.

**Goal:** `spawn_on(cpu, task)` — spawn a task onto ANOTHER core's per-core executor from
any core. This is the piece that finally EXERCISES the Step-2 cross-core wake end-to-end
(`__pender` per-core → `wake_core` → targeted IPI): the Step-2 inbox round-trip used a
noop waker + inline poll, so the cross-core wake path was built but not exercised. Spec
§8; follows 3b (per-core executors via `run_core`, committed + verified).

**Approach (embassy 0.6.3):** use embassy's `SendSpawner` (the intended cross-thread
spawn handle). Each core's `run_core` publishes `exec.spawner().make_send()` into a
per-core slot. `spawn_on(cpu, token)` calls `PER_CORE_SPAWNER[cpu].spawn(token)`:
embassy enqueues the task onto core `cpu`'s run-queue (atomic intrusive list, cross-core
safe) and calls the pender with that executor's context (= `cpu`, set in 3b's
`run_core`) → our `__pender` → `wake_core(cpu)` → targeted `VEC_WAKE` IPI → core `cpu`
leaves `hlt` and polls the new task. No hand-rolled queue/type-erasure needed.

**Prerequisites (committed):** Step 2 (per-core `__pender`/`WAKE_PENDING`, `wake_core`,
targeted `send_ipi`), 3b (`PER_CORE_EXECUTOR` + `run_core(cpu)` with context = cpu).

**Verify first:** embassy-executor 0.6.3 exposes `embassy_executor::SendSpawner` and
`Spawner::make_send(&self) -> SendSpawner`, and `SendSpawner::spawn<S: Send>(SpawnToken<S>)`.
Confirm by reading the embassy docs/source in the cargo registry (or just compile — a
missing symbol fails fast). If `make_send`/`SendSpawner` is absent in this version,
STOP and report (fallback would be a hand-rolled per-core spawn queue carrying a
type-erased token — more work; ask before doing it).

---

## File Structure
- `kernel/src/executor/mod.rs` — `PER_CORE_SPAWNER[MAX_CPUS]`; `run_core` publishes
  `make_send()` after building the executor; `pub fn spawn_on<S: Send>(cpu, token)`.
- `kernel/src/boot/phases/interrupts.rs` — 3c boot-check: BSP `spawn_on(1, probe)` →
  `SPAWN_RAN_ON == 1`.
- `CHANGELOG/NN`.

---

## Task 1: `PER_CORE_SPAWNER` + `spawn_on`

**Files:** `kernel/src/executor/mod.rs`

- [ ] **Step 1: import + per-core spawner slots** — Add `use embassy_executor::SendSpawner;`
  (verify the path; in 0.6 it's `embassy_executor::SendSpawner`). Add:
```rust
/// Each core's SendSpawner (published by run_core once its executor exists). A core
/// can spawn onto another via PER_CORE_SPAWNER[target].spawn(token) — embassy enqueues
/// on the target's run-queue (atomic, cross-core safe) and pends it (→ __pender(target)
/// → wake_core(target) → targeted IPI). None until that core has entered run_core.
static PER_CORE_SPAWNER: [IrqMutex<Option<SendSpawner>>; crate::cpu::MAX_CPUS] = {
    const S: IrqMutex<Option<SendSpawner>> = IrqMutex::new(None);
    [S; crate::cpu::MAX_CPUS]
};
```
(Confirm `SendSpawner: Send + Copy` so it stores in the static cleanly; if not Copy,
store it anyway behind the Option — `make_send` returns it by value.)

- [ ] **Step 2: publish in run_core** — In `run_core(cpu)`, AFTER `let spawner =
  exec.spawner();` and BEFORE the spawn block, publish this core's send-spawner:
```rust
    *PER_CORE_SPAWNER[cpu as usize].lock() = Some(spawner.make_send());
```

- [ ] **Step 3: `spawn_on`** — Add:
```rust
/// Spawn `token` onto core `cpu`'s executor from any core. Errors if `cpu` hasn't
/// entered run_core yet (no spawner published) or the task pool is exhausted.
pub fn spawn_on<S: Send>(cpu: u32, token: embassy_executor::SpawnToken<S>)
    -> Result<(), embassy_executor::SpawnError>
{
    let g = PER_CORE_SPAWNER[cpu as usize].lock();
    match g.as_ref() {
        Some(s) => s.spawn(token),
        None => Err(embassy_executor::SpawnError::Busy), // not ready; caller may retry
    }
}
```
(Check the exact `SpawnError` variants in 0.6 — use whatever represents "can't spawn";
`Busy` is the pool-exhausted variant. If there's no good "not ready" variant, return the
pool-exhausted one or define the contract as "returns Err if target not ready".)

- [ ] **Step 4: build** — `make test-boot` (1 core). Expected `TEST_BOOT_PASS` (nothing
  calls spawn_on yet; this just compiles the new API + the make_send publish on the BSP).
  If `make_send`/`SendSpawner`/`SpawnToken` paths are wrong, fix per the real 0.6 API.

- [ ] **Step 5: commit** —
```
git add kernel/src/executor/mod.rs
git commit -m "feat(smp): 3c — cross-core spawn_on via embassy SendSpawner (Step 3c part 1)"
```
Trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## Task 2: 3c gate — BSP spawns onto AP1, task runs on core 1

**Files:** `kernel/src/executor/mod.rs` (probe task), `kernel/src/boot/phases/interrupts.rs`, `CHANGELOG/NN`

- [ ] **Step 1: probe task** — In `executor/mod.rs`, under boot-checks:
```rust
#[cfg(feature = "boot-checks")]
pub static SPAWN_RAN_ON: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(u32::MAX);

#[cfg(feature = "boot-checks")]
#[embassy_executor::task]
async fn cross_spawn_probe() {
    // Records the core this task actually RAN on. If spawn_on(1, ..) worked, this is 1.
    SPAWN_RAN_ON.store(crate::cpu::cpu_id(), core::sync::atomic::Ordering::SeqCst);
}
```

- [ ] **Step 2: boot-check** — In `interrupts.rs` `#[cfg(feature="boot-checks")]` after
  bringup (after the 3b heartbeat check), spawn the probe onto core 1 and confirm it ran
  there:
```rust
    #[cfg(feature = "boot-checks")]
    if crate::cpu::cpus_online() >= 2 {
        // Retry spawn until core 1 has published its SendSpawner (it enters run_core
        // during bringup, so usually immediately).
        let mut spawned = false;
        for _ in 0..1_000_000u64 {
            if crate::executor::spawn_on(1, crate::executor::cross_spawn_probe()).is_ok() { spawned = true; break; }
            core::hint::spin_loop();
        }
        // Wait for the probe to run on core 1 (it's woken via the cross-core IPI).
        let mut ran_on = u32::MAX;
        for _ in 0..50_000_000u64 {
            let v = crate::executor::SPAWN_RAN_ON.load(core::sync::atomic::Ordering::SeqCst);
            if v != u32::MAX { ran_on = v; break; }
            core::hint::spin_loop();
        }
        crate::binfo!("exec", "cross-spawn ran_on=core{} (spawned={}, expect core1)", ran_on, spawned);
    }
```
> NOTE: `cross_spawn_probe` must be `pub` (called from interrupts.rs). The `#[task]` macro
> wrapper fn returns the `SpawnToken` — make it `pub`. The probe future is Send (no
> non-Send captures) so `SendSpawner::spawn` accepts it.

- [ ] **Step 3: build + run -smp 4 (the gate), TWICE** —
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso CARGO_FEATURES="boot-checks" && for i in 1 2; do echo "run $i"; timeout 90 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -m 512 -no-reboot -display none -serial stdio -device qemu-xhci -cdrom build/os.iso 2>&1 | grep -E "cross-spawn|APs online"; done'
```
GATE: `cross-spawn ran_on=core1 (spawned=true, expect core1)`, stable across BOTH runs.
- `ran_on=core1` ⇒ the task spawned by the BSP actually RAN on core 1 → cross-core spawn
  + the cross-core wake (`__pender`→`wake_core(1)`→IPI) worked end-to-end. THE KEY PROOF.
- `ran_on=core0` ⇒ it ran on the BSP (spawn went to the wrong executor) → bug.
- `spawned=false` or `ran_on=255` (u32::MAX) ⇒ spawn failed or the task never ran (core 1
  not woken) → bug. Do NOT mark 3c done unless `ran_on=core1`.
Also `make test-boot` (1 core) → `TEST_BOOT_PASS` (check skipped on 1 core), and
`make run-smp-test` + `make run-smp2-test` → PASS (executors + pool intact).

- [ ] **Step 4: changelog + commit** — next free number. Cosa: 3c — `spawn_on` via
  SendSpawner; boot-check proves a BSP-spawned task runs on core 1 (cross-core spawn +
  cross-core wake end-to-end). Perché: completes the message/spawn fabric; Step 5 (pin
  GUI) can spawn the compositor onto the GUI core, and the general model can distribute
  WASM apps across cores.
```
git add kernel/src/executor/mod.rs kernel/src/boot/phases/interrupts.rs CHANGELOG/NN-...
git commit -m "test(smp): 3c — cross-core spawn boot-check (BSP spawns task that runs on core 1)"
```
Trailer as above.

---

## Self-Review
- **Exercises the Step-2 cross-core wake for real:** unlike the Step-2 inbox round-trip
  (noop waker + inline poll), here core 1 is asleep in `run_core`'s `hlt` and is woken by
  the spawn's pend → `wake_core(1)` → targeted IPI → polls the new task. `ran_on=core1`
  is the proof both the spawn AND the wake worked.
- **Cross-core enqueue safety:** embassy's run-queue is an atomic intrusive list designed
  for `SendSpawner` cross-thread use — the enqueue from the BSP onto core 1's queue is
  lock-free/atomic. We rely on embassy's own SMP-safe primitive, not a hand-rolled queue.
- **No new IPI vector:** reuses `VEC_WAKE` (0x40) via `wake_core` (Step 2). No new handler.
- **Single-writer executor preserved:** the BSP never touches core 1's RawExecutor
  directly — `SendSpawner` does the atomic enqueue; core 1 still solely polls its own
  executor. The 3b Sync invariant holds.
- **Risk:** the `make_send`/`SendSpawner`/`SpawnError` API surface of embassy 0.6.3 must
  match (Step 1 verifies by compiling). The retry loop on `spawn_on` handles the (tiny)
  window where core 1 hasn't published its spawner yet. If `ran_on` is `core0` the spawn
  routed locally (wrong) — investigate `make_send`'s executor binding. Do NOT mark done
  unless `ran_on=core1` on both runs.
