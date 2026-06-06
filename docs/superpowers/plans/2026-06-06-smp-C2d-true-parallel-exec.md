# C2d — True parallel `.cwasm` execution (remove the serialization lock) Implementation Plan

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development / executing-plans. `- [ ]`.
> ⚠️ HIGH-RISK: removes the global lock that today makes concurrent wasmtime safe.
> Correctness depends on (1) wasmtime's `custom-sync-primitives` feature wired to real
> cross-core locks AND (2) per-core TLS. BOTH are required — one without the other is UB.

**Goal:** two (or more) `.cwasm` apps run on two (or more) ComputeApp cores AT THE SAME
WALL-CLOCK TIME — the real general-throughput win. Today C2c routes each `.cwasm` exec to a
core but `RUN_CWASM_LOCK` (a `spin::Mutex` in `wasm/wt/mod.rs`) serializes every
`run_cwasm` call because no_std wasmtime's default sync primitives PANIC on concurrent
lock contention (`sync_nostd.rs::panic_on_contention`). C2d removes that bottleneck.

**Root cause (verified in the wasmtime 45.0.0 source):**
`~/.cargo/registry/src/*/wasmtime-45.0.0/src/sync_nostd.rs` — without the
`custom-sync-primitives` feature, wasmtime's internal `Mutex`/`RwLock` are
`panic_on_contention`: the second concurrent locker calls
`panic!("concurrent lock request, must use std or custom-sync-primitives ...")`.
With `custom-sync-primitives` enabled (build.rs sets `has_custom_sync = !std &&
custom-sync-primitives && runtime`), wasmtime instead calls 8 embedder-provided extern
"C" functions (declared in `src/runtime/vm/sys/custom/capi.rs`):

```
fn wasmtime_sync_lock_acquire(lock: *mut usize);
fn wasmtime_sync_lock_release(lock: *mut usize);
fn wasmtime_sync_lock_free(lock: *mut usize);
fn wasmtime_sync_rwlock_read(lock: *mut usize);
fn wasmtime_sync_rwlock_read_release(lock: *mut usize);
fn wasmtime_sync_rwlock_write(lock: *mut usize);
fn wasmtime_sync_rwlock_write_release(lock: *mut usize);
fn wasmtime_sync_rwlock_free(lock: *mut usize);
```

Each call gets a `*mut usize` — an 8-byte cell wasmtime embeds inline in its lock struct,
zero-initialized (`UnsafeCell::new(0)`). The embedder may store lock state inline in that
word (no allocation, no real free work). wasmtime's locks are FINE-GRAINED (brief type/
module-registry inserts) — NOT held during guest execution — so real locks here give true
parallel guest execution with only brief registry contention.

**Second required change — per-core TLS.** `wasm/wt/platform.rs` currently backs
`wasmtime_tls_get/set` with a SINGLE global `AtomicPtr` ("all wasm on the BSP ... one
global pointer suffices"). wasmtime stores per-activation call/trap state (`CallThreadState`)
in TLS. With concurrent execution on cores 2+3 a single shared TLS pointer is corrupted
across cores. MUST become a per-core array indexed by `cpu_id()`. (Today it is only safe
because `RUN_CWASM_LOCK` guarantees one core in wasmtime at a time.)

**Deadlock analysis (done — record so the implementer doesn't reintroduce a hazard):**
The mmap shims (`wasmtime_mmap_new`/`_remap`/`_munmap`/`_mprotect`) take the kernel `MAPPER`
(`spin::Mutex`) only INSIDE `map_page`/`set_flags`/`unmap_page` — per page, acquired and
released immediately, NEVER held across any wasmtime-registry-lock code. `allocate_frame`/
`free_frame` are likewise leaf locks. So the lock order is strictly `wasmtime-reglock ⊃
MAPPER`, never the reverse → no ABBA deadlock. Keep it that way: do NOT hold MAPPER across
a wasmtime call, and do NOT call wasmtime from inside a MAPPER critical section. The custom
sync locks must spin with INTERRUPTS ENABLED (never mask IF) so a spinning core still ACKs
TLB-shootdown IPIs (Step 3d) and services its timer — masking IF here would deadlock the
shootdown ack-wait.

**Prerequisites (committed):** C2c (per-request reply + routing + `RUN_CWASM_LOCK`), C2b/C2a/C1
(run_cwasm on an AP), 3b/3c (per-core executors, spawn_on), 3d (TLB shootdown — interacts
with the spin-with-IF-on requirement), 1a fast `cpu_id` (used in per-core TLS hot path).

**CHANGELOG:** next free on this branch = **319**. Trailer:
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## Task 1: enable `custom-sync-primitives` + implement the 8 sync shims + per-core TLS

**Files:** `kernel/Cargo.toml`, `kernel/src/wasm/wt/platform.rs`

- [ ] **Step 1: enable the wasmtime feature** — In `kernel/Cargo.toml` line 23, add
  `custom-sync-primitives` to the wasmtime feature list:
```toml
wasmtime = { version = "=45.0.0", default-features = false, features = ["runtime", "custom-virtual-memory", "custom-sync-primitives", "component-model"] }
```
  (build.rs: `has_custom_sync = !std && custom-sync-primitives && runtime` → flips on. This
  makes wasmtime expect the 8 `wasmtime_sync_*` symbols at link time — Step 2 provides them.)

- [ ] **Step 2: implement the 8 sync shims** — In `kernel/src/wasm/wt/platform.rs`, add a
  new section. State is stored INLINE in the `*mut usize` cell (treated as `AtomicUsize`;
  `usize` and `AtomicUsize` share layout/alignment, and wasmtime zero-inits the cell, which
  is our "unlocked" state). All spins keep INTERRUPTS ENABLED (no `cli`) — required for the
  TLB-shootdown ack (3d) and forward progress.
```rust
// ---------------------------------------------------------------------------
// Custom sync primitives (`custom-sync-primitives` feature). no_std Wasmtime's
// default locks PANIC on contention; with this feature it calls these shims so
// multiple cores can run wasm concurrently. State lives inline in the 8-byte
// cell Wasmtime hands us (zero-init = unlocked). We spin with IRQs ENABLED so a
// waiting core still services timer + TLB-shootdown IPIs (no `cli` here).
// Locks are non-reentrant (matches std Mutex/RwLock semantics Wasmtime assumes).
// ---------------------------------------------------------------------------
use core::sync::atomic::AtomicUsize;

#[inline]
fn lock_cell(lock: *mut usize) -> &'static AtomicUsize {
    // SAFETY: Wasmtime guarantees `lock` points to a live, 8-byte-aligned cell
    // it zero-initialized and uses only via these shims for its lifetime.
    unsafe { &*(lock as *const AtomicUsize) }
}

#[no_mangle]
pub extern "C" fn wasmtime_sync_lock_acquire(lock: *mut usize) {
    let a = lock_cell(lock); // 0 = unlocked (zero-init), 1 = locked
    while a
        .compare_exchange_weak(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub extern "C" fn wasmtime_sync_lock_release(lock: *mut usize) {
    lock_cell(lock).store(0, Ordering::Release);
}

#[no_mangle]
pub extern "C" fn wasmtime_sync_lock_free(_lock: *mut usize) {
    // Inline state — nothing to free.
}

/// RwLock encoding in the cell: 0 = free, 1..=(MAX-1) = N readers,
/// usize::MAX = one writer (exclusive). Reader count never approaches MAX
/// (≤ MAX_CPUS concurrent readers), so the sentinel is unambiguous.
const RW_WRITER: usize = usize::MAX;

#[no_mangle]
pub extern "C" fn wasmtime_sync_rwlock_read(lock: *mut usize) {
    let a = lock_cell(lock);
    loop {
        let s = a.load(Ordering::Relaxed);
        if s != RW_WRITER
            && a.compare_exchange_weak(s, s + 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        {
            return;
        }
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub extern "C" fn wasmtime_sync_rwlock_read_release(lock: *mut usize) {
    lock_cell(lock).fetch_sub(1, Ordering::Release);
}

#[no_mangle]
pub extern "C" fn wasmtime_sync_rwlock_write(lock: *mut usize) {
    let a = lock_cell(lock);
    while a
        .compare_exchange_weak(0, RW_WRITER, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub extern "C" fn wasmtime_sync_rwlock_write_release(lock: *mut usize) {
    lock_cell(lock).store(0, Ordering::Release);
}

#[no_mangle]
pub extern "C" fn wasmtime_sync_rwlock_free(_lock: *mut usize) {
    // Inline state — nothing to free.
}
```

- [ ] **Step 3: per-core TLS** — In `platform.rs`, REPLACE the single global TLS with a
  per-core array. Change the existing block (lines ~20-33):
```rust
// ---------------------------------------------------------------------------
// TLS — one pointer PER CORE. Wasmtime stores per-activation CallThreadState
// here; with concurrent execution on multiple cores a single global pointer
// would be corrupted across cores, so index by cpu_id(). cpu_id() is the fast
// RDTSCP path (~23 ns) — cheap enough for the tls_get/set hot path.
// ---------------------------------------------------------------------------
use crate::cpu::MAX_CPUS;

static TLS: [AtomicPtr<u8>; MAX_CPUS] = {
    const Z: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
    [Z; MAX_CPUS]
};

#[no_mangle]
pub extern "C" fn wasmtime_tls_get() -> *mut u8 {
    TLS[crate::cpu::cpu_id() as usize].load(Ordering::SeqCst)
}

#[no_mangle]
pub extern "C" fn wasmtime_tls_set(ptr: *mut u8) {
    TLS[crate::cpu::cpu_id() as usize].store(ptr, Ordering::SeqCst);
}
```
  (Confirm `MAX_CPUS` is `pub` in `cpu/mod.rs` — it is, `pub const MAX_CPUS: usize = 16`.
  Confirm `cpu_id()` returns a value usable as `usize` index ≤ MAX_CPUS-1.)

- [ ] **Step 4: build (1 core, default + boot-checks)** —
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso && make iso CARGO_FEATURES="boot-checks"'
```
  Expected: both build clean. A link error for a missing `wasmtime_sync_*` symbol means a
  shim name/signature is wrong. (Do NOT boot yet — Task 2 removes the serialization lock.)

- [ ] **Step 5: commit** —
```
git add kernel/Cargo.toml kernel/src/wasm/wt/platform.rs
git commit -m "feat(smp): C2d — wasmtime custom-sync-primitives shims + per-core TLS (enable concurrent wasm)"
```

## Task 2: remove `RUN_CWASM_LOCK` (let cores run wasm concurrently)

**Files:** `kernel/src/wasm/wt/mod.rs`

- [ ] **Step 1: drop the serialization guard** — In `run_cwasm` (mod.rs:175), remove
  `let _guard = RUN_CWASM_LOCK.lock();` (line 176). Remove the `static RUN_CWASM_LOCK`
  (line 167) and its doc comment (lines ~152-166). Update the `run_cwasm` doc comment (lines
  ~169-174) to state that concurrency is now handled by wasmtime's custom-sync-primitives +
  the per-core TLS in `platform.rs` (no global lock).

- [ ] **Step 2: build (1 core)** —
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso'
```
  Expected: clean (no remaining references to `RUN_CWASM_LOCK`).

- [ ] **Step 3: regression — run-test (1 core, CRITICAL)** —
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test'
```
  Expected `TEST_PASS`. On 1 core, `pick_compute_core()` returns `None` → `.cwasm` runs
  inline (single core never contends the new locks) — proves the shims don't break the
  baseline. A panic mentioning "concurrent lock request" here would mean a shim is wrong
  (or the feature didn't actually enable) — investigate before proceeding.

- [ ] **Step 4: commit** —
```
git add kernel/src/wasm/wt/mod.rs
git commit -m "feat(smp): C2d — remove RUN_CWASM_LOCK; concurrency now via custom-sync + per-core TLS"
```

## Task 3: a CPU-heavy `spin.cwasm` + a TRUE-overlap gate (replaces the misleading compute probe)

**Files:** `tools/wt-spin/spin.wat` (new), `Makefile`, `kernel/src/wasm/wt/mod.rs` (embed +
runner), `kernel/src/executor/mod.rs` (rewrite `parallel_probe` to call run_cwasm),
`kernel/src/boot/phases/interrupts.rs` (fix the gate comments), `CHANGELOG/319-...`

> WHY: C2c's `parallel_probe` is a PURE-COMPUTE loop (200M arith ops), NOT `run_cwasm`. Its
> `parallel-exec overlap=true` marker proves compute-parallelism (the kernel already had
> that) — it never exercised wasmtime concurrency. The interrupts.rs comment even falsely
> claims "3 × run_echo_demo per probe". C2d makes the probe actually run `run_cwasm` on a
> CPU-heavy `.cwasm`, so `overlap=true` becomes the REAL proof of parallel wasm execution.

- [ ] **Step 1: spin guest** — Create `tools/wt-spin/spin.wat`: a WASI command (`_start`
  export) that busy-loops a tuned count then returns 0. No imports needed (WASI linker
  supplies them; unused). Tune `LIMIT` so a single run is ~300-800 ms on QEMU (measurable):
```wat
(module
  ;; WASI command entry. Busy-loops to consume CPU, then returns (exit 0).
  (func (export "_start")
    (local $i i64)
    (local.set $i (i64.const 0))
    (block $done
      (loop $spin
        (local.set $i (i64.add (local.get $i) (i64.const 1)))
        ;; LIMIT: tune for ~300-800 ms/run on QEMU -cpu max. Start at 2e9.
        (br_if $done (i64.ge_u (local.get $i) (i64.const 2000000000)))
        (br $spin)))))
```

- [ ] **Step 2: Makefile rule** — mirror the `hello.cwasm` rule (Makefile ~109). Add to the
  `WT_KCWASMS` list and add the rule:
```makefile
$(WT_KDIR)/spin.cwasm: tools/wt-spin/spin.wat $(WT_PRECOMPILE)
	$(WT_PRECOMPILE) tools/wt-spin/spin.wat $(WT_KDIR)/spin.cwasm
```
  (Match the exact recipe form of the `hello.cwasm`/`echo.cwasm` rules — check whether they
  pass the `.wat` directly to `$(WT_PRECOMPILE)` or convert first, and copy that form. Add
  `$(WT_KDIR)/spin.cwasm` to the `WT_KCWASMS :=` list so `wt-cwasm` builds it.)

- [ ] **Step 3: embed + runner** — In `kernel/src/wasm/wt/mod.rs`, under boot-checks, embed
  the bytes and add a runner mirroring `run_echo_demo`:
```rust
#[cfg(feature = "boot-checks")]
static SPIN_CWASM: &[u8] = include_bytes!("spin.cwasm");

/// Boot-check: run the CPU-heavy spin guest via the REAL run_cwasm path
/// (WASI linker + instantiate + execute). Returns the guest exit code (0).
#[cfg(feature = "boot-checks")]
pub fn run_spin_demo() -> i32 {
    run_cwasm(SPIN_CWASM, alloc::vec::Vec::new(), None)
}
```

- [ ] **Step 4: rewrite `parallel_probe` to call run_cwasm** — In
  `kernel/src/executor/mod.rs` (the `parallel_probe` task, ~line 411), REPLACE the pure-
  compute loop body with `iters` runs of `run_spin_demo()`. Keep the same statics
  (`PARALLEL_ACC`/`PARALLEL_RAN`/`PARALLEL_DONE`) and signature so the interrupts.rs gate
  is unchanged structurally. Update the doc comment to state it now exercises run_cwasm:
```rust
/// C2d parallelism probe: runs the CPU-heavy spin `.cwasm` via run_cwasm
/// `iters` times on whatever core embassy scheduled it on. Two of these on
/// cores 2 and 3 concurrently => if wall ≈ single-run, wasm ran in PARALLEL
/// (custom-sync-primitives + per-core TLS working). This is THE real proof.
#[cfg(feature = "boot-checks")]
#[embassy_executor::task(pool_size = 2)]
pub async fn parallel_probe(idx: u32, iters: u32) {
    let mut last: i32 = 0;
    for _ in 0..iters {
        last = crate::wasm::wt::run_spin_demo();
    }
    PARALLEL_ACC[idx as usize].store(last as u64, core::sync::atomic::Ordering::SeqCst);
    PARALLEL_RAN[idx as usize].store(
        crate::cpu::cpu_id(),
        core::sync::atomic::Ordering::SeqCst,
    );
    PARALLEL_DONE.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
}
```
  (Keep `ITERS` in interrupts.rs small — 1 or 2 — since each iter is now a full ~300-800 ms
  run_cwasm, not cheap arithmetic. Adjust the `ITERS` const + update the interrupts.rs gate
  comments lines ~487-514 to say "run_cwasm(spin.cwasm)" instead of the false
  "run_echo_demo"/compute-loop wording. The overlap threshold ≤1.6× stays.)

- [ ] **Step 5: THE GATE — build boot-checks + boot -smp 4, TWICE** —
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso CARGO_FEATURES="boot-checks" && for i in 1 2; do echo "== run $i =="; timeout 120 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -m 512 -no-reboot -display none -serial stdio -device qemu-xhci -cdrom build/os.iso 2>&1 | grep -E "parallel-exec|#DF|#PF|#GP|panic|concurrent lock"; done'
```
  GATE (the REAL throughput proof): `parallel-exec ran=[2,3] concurrent_ms=X single_ms=Y
  overlap=true` with `X ≈ Y` (NOT ≈ 2Y), STABLE on both runs, NO `panic`/`#DF`/`concurrent
  lock`. Because the probe now runs `run_cwasm`, `overlap=true` means TWO wasm modules
  executed in PARALLEL on cores 2+3 = the throughput win. `ran=[2,3]` = distinct cores.
  - A `panic "concurrent lock request"` ⇒ the feature/shims aren't actually active (Task 1
    wrong) — concurrent wasmtime hit the panic_on_contention path.
  - `overlap=false` (concurrent ≈ 2×single) ⇒ still serialized — a remaining global lock or
    a contended fine-grained lock dominating; investigate which wasmtime lock.
  - `#DF`/`#PF` ⇒ likely TLS still shared (Task 1 Step 3 wrong) or AP stack — re-check.

- [ ] **Step 6: full regression suite** — ALL must pass:
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test && make run-smp-test && make run-smp2-test && make run-ssh-gui-test && make run-exec-ap-test'
```
  Expected: `TEST_PASS`, `TEST_PASS_SMP`, smp2 PASS, ssh-gui PASS, `TEST_PASS_EXEC_AP`.
  (run-exec-ap-test now runs a `.cwasm` on a core with the locks live + no global lock —
  the routed single exec must still work.)

- [ ] **Step 7: changelog + commit** — Create `CHANGELOG/319-26-06-06-c2d-true-parallel-exec.md`
  documenting: enabled custom-sync-primitives, the 8 shims + per-core TLS, removed
  RUN_CWASM_LOCK, the deadlock analysis, the honest gate (run_cwasm overlap). Then:
```
git add tools/wt-spin/spin.wat Makefile kernel/src/wasm/wt/mod.rs kernel/src/executor/mod.rs kernel/src/boot/phases/interrupts.rs CHANGELOG/319-26-06-06-c2d-true-parallel-exec.md
git commit -m "test(smp): C2d — spin.cwasm + TRUE parallel-exec gate (run_cwasm overlap on cores 2+3)"
```

---

## Self-Review / risks
- **Both-or-nothing:** custom-sync alone (without per-core TLS) → TLS corruption under
  concurrency; per-core TLS alone (without custom-sync) → panic_on_contention. Task 1 does
  both before Task 2 removes the lock. Do NOT boot -smp >1 between them.
- **Spin-with-IF-on:** the shims must NOT mask interrupts — a core spinning on a wasmtime
  lock must still ACK TLB-shootdown IPIs (3d) or the shootdown ack-wait deadlocks. The code
  above uses plain atomics + `spin_loop()` (no `cli`) — keep it so.
- **Deadlock order:** `wasmtime-reglock ⊃ MAPPER`, never reverse (mmap shims hold MAPPER
  only per-page inside map_page, never across wasmtime calls). Don't introduce a path that
  holds MAPPER across a wasmtime call.
- **Non-reentrant locks:** matches std Mutex/RwLock (wasmtime works with std → no
  reentrancy). If a future wasmtime version recursively locks on one thread it would
  deadlock — out of scope for 45.0.0.
- **The gate is the whole point:** C2c's gate measured compute, not wasm — that's why it
  was misleading. C2d's gate MUST call `run_cwasm` (via run_spin_demo). Do NOT revert to a
  compute loop. `overlap=true` with a run_cwasm body = real parallel wasm. Verify TWICE
  (QEMU timing is noisy).
- **Stack:** two concurrent run_cwasm run on TWO DIFFERENT cores' poll stacks (one each) —
  same per-core profile as C2a/C2b (fit 65536). No new single-stack pressure.
