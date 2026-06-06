# C2b — Route .cwasm shell-exec to a ComputeApp core Implementation Plan

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development / executing-plans. `- [ ]`.

**Goal:** a `.cwasm` app `exec`'d from a shell runs on a ComputeApp core (not the BSP), so
the BSP executor stays free for I/O during the app run. Single-slot exec still (serialized
— parallelism is C2c), but the app is OFF the BSP. Builds on C2a (run_cwasm on an AP, proven)
+ 3c (spawn_on) + Step 2 (cross-core wake for the completion).

**Scope:** ONLY `.cwasm` (wasmtime AOT) regular apps (NOT the compositor — it has its own
hand-off, Step 5). `.wasm` (wasmi, a different runtime) stays on the BSP for now.

**Stack:** keep the spawned task's stack ≈ C2a's (which fit 65536): do the VFS `read_all`
+ `proc::register` on the BSP exec_worker; the spawned task ONLY runs `run_cwasm(&bytes,
argv, pts)`. If a #DF still appears, bump `task-arena-size` (per C2a's note).

**Prerequisites (committed):** C2a (run_cwasm on AP), 3c (`spawn_on`), Step 2 (`wake_core`),
3b (per-core executors). Global PTY/proc locks SMP-safe (Step 7) → app stdio/pid cross-core safe.

**CHANGELOG:** next free on this branch. Trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## File Structure
- `kernel/src/executor/mod.rs` — `AppReply` static + `AppReplyFuture`; `run_app_on_core`
  task (moved bytes/argv/pts → run_cwasm → fill reply); `first_compute_app_core()` helper;
  exec_worker_task routes the `.cwasm` non-compositor run.
- `kernel/src/cpu/mod.rs` — (maybe) a helper to find the first online ComputeApp core.
- `user-bin/exec-ap-init.sh` (new) + `Makefile` target `run-exec-ap-test` — the gate.
- `CHANGELOG/NN`.

---

## Task 1: reply slot + run_app_on_core task + routing

**Files:** `kernel/src/executor/mod.rs` (+ `cpu/mod.rs` helper)

- [ ] **Step 1: AppReply (single-slot, reuse Step-2 ReplySlot shape)** — In executor/mod.rs:
```rust
use core::sync::atomic::{AtomicI32, AtomicBool, Ordering};
struct AppReply { code: AtomicI32, done: AtomicBool, waker: crate::sync::IrqMutex<Option<core::task::Waker>> }
static APP_REPLY: AppReply = AppReply { code: AtomicI32::new(0), done: AtomicBool::new(false), waker: crate::sync::IrqMutex::new(None) };
impl AppReply {
    fn arm(&self) { self.done.store(false, Ordering::SeqCst); }
    fn complete(&self, code: i32) {
        self.code.store(code, Ordering::SeqCst);
        self.done.store(true, Ordering::SeqCst);            // Release of code
        if let Some(w) = self.waker.lock().take() { w.wake(); } // cross-core → BSP
    }
}
struct AppReplyFuture;
impl core::future::Future for AppReplyFuture {
    type Output = i32;
    fn poll(self: core::pin::Pin<&mut Self>, cx: &mut core::task::Context<'_>) -> core::task::Poll<i32> {
        if APP_REPLY.done.load(Ordering::SeqCst) { return core::task::Poll::Ready(APP_REPLY.code.load(Ordering::SeqCst)); }
        *APP_REPLY.waker.lock() = Some(cx.waker().clone());
        if APP_REPLY.done.load(Ordering::SeqCst) { core::task::Poll::Ready(APP_REPLY.code.load(Ordering::SeqCst)) } else { core::task::Poll::Pending }
    }
}
```
(Single-slot: one routed exec at a time — matches the single-slot EXEC_QUEUE. C2c makes it per-core.)

- [ ] **Step 2: run_app_on_core task** — runs run_cwasm on whatever core it's spawned on:
```rust
#[embassy_executor::task(pool_size = 1)]
async fn run_app_on_core(bytes: alloc::boxed::Box<[u8]>, argv: alloc::vec::Vec<alloc::vec::Vec<u8>>, pts: usize) {
    let cpu = crate::cpu::cpu_id();
    let code = crate::wasm::wt::run_cwasm(&bytes, argv, Some(pts));
    crate::binfo!("exec-ap", "ran_on=core{} code={}", cpu, code);
    APP_REPLY.complete(code);
}
```
> `bytes`/`argv` are owned (Send) → ok as task args. `pool_size=1` (single in-flight; C2c
> bumps it). The task runs `run_cwasm` synchronously on this core's poll stack (C2a proved
> it fits). proc::register/unregister + VFS read stay on the BSP (Step 3) → this task's
> stack ≈ C2a's.

- [ ] **Step 3: first_compute_app_core()** — In cpu/mod.rs:
```rust
/// First online ComputeApp core, or None (then exec runs inline on the BSP).
pub fn first_compute_app_core() -> Option<u32> {
    let total = 1 + cpus_online();
    (1..total).find(|&c| core_role(c) == CoreRole::ComputeApp)
}
```

- [ ] **Step 4: route in exec_worker_task** — In the `.cwasm` branch, AFTER the compositor
  special-case + AFTER `let pid = proc::register(...)`, replace the inline
  `let c = run_cwasm(&bytes, slot.argv, Some(slot.term_pts));` with:
```rust
    let c = match crate::executor::first_compute_app_core() {
        Some(core) => {
            APP_REPLY.arm();
            // Move bytes/argv to the AP task; it runs run_cwasm there + completes APP_REPLY.
            let boxed = bytes.into_boxed_slice();
            match crate::executor::spawn_on(core, run_app_on_core(boxed, slot.argv, slot.term_pts)) {
                Ok(()) => AppReplyFuture.await,           // BSP executor keeps polling I/O
                Err(_) => { /* pool busy: fall back inline */ run_cwasm_inline_fallback }, // see note
            }
        }
        None => crate::wasm::wt::run_cwasm(&bytes, slot.argv, Some(slot.term_pts)), // 1-2 core: inline
    };
```
> NOTE the borrow: `bytes.into_boxed_slice()` MOVES bytes — but the `None` arm needs
> `&bytes`. Restructure so the read `bytes` is consumed in exactly one path (e.g. compute
> `first_compute_app_core()` first; if Some, move bytes into the task; if None, run inline
> with `&bytes`). And the `Err(_)` spawn-fail (pool_size=1 busy) fallback: since exec is
> single-slot (one at a time), the pool should never be busy — but handle `Err` by running
> inline as a safety net. You'll need to keep `bytes` available for the inline fallback —
> simplest: on the route path, if spawn fails, you've already moved bytes; so check
> `spawn_on`-readiness differently OR clone bytes for the fallback. Cleanest: since
> single-slot guarantees the pool is free, treat `Err` as a hard error (log + return 127).

- [ ] **Step 5: build** — `make test-boot` (1 core: `first_compute_app_core()` = None →
  inline fallback = today's path). Expected `TEST_BOOT_PASS`. The `.cwasm` execs in any
  1-core test still run inline.

- [ ] **Step 6: commit** —
```
git add kernel/src/executor/mod.rs kernel/src/cpu/mod.rs
git commit -m "feat(smp): C2b — route .cwasm shell-exec to a ComputeApp core (app off the BSP)"
```
Trailer as above.

---

## Task 2: gate — a .cwasm exec runs on a ComputeApp core

**Files:** `user-bin/exec-ap-init.sh` (new), `Makefile`, `CHANGELOG/NN`

- [ ] **Step 1: init script** — `user-bin/exec-ap-init.sh`: run a `.cwasm` tool that
  prints a marker, so exec_worker routes it. `wtecho` resolves to /bin/wtecho.cwasm (the
  Makefile stages it). Content:
```sh
wtecho EXEC_AP_OK
```
(Optionally follow with the normal greeter so the boot completes the HELLO sentinel.)

- [ ] **Step 2: Makefile target** —
```makefile
.PHONY: run-exec-ap-test
run-exec-ap-test:
	@$(MAKE) iso INIT_SCRIPT=user-bin/exec-ap-init.sh
	@echo "--- exec-on-AP (-smp 4) ---"
	@timeout 90 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -m 512 -no-reboot -display none -serial stdio \
	  -device qemu-xhci -cdrom $(ISO) 2>&1 | tee build/exec-ap.log; \
	grep -qE "exec-ap ran_on=core[1-9]" build/exec-ap.log || { echo TEST_FAIL_EXEC_AP_CORE; exit 1; }; \
	grep -qF "EXEC_AP_OK" build/exec-ap.log || { echo TEST_FAIL_EXEC_AP_OUTPUT; exit 1; }; \
	echo TEST_PASS_EXEC_AP
```
(`exec-ap ran_on=core[1-9]` = ran on a non-BSP core. `EXEC_AP_OK` = wtecho's output
reached serial via the shell PTY → proves the app ran + its stdout works cross-core.)

- [ ] **Step 3: run the gate** —
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-exec-ap-test'
```
GATE: `TEST_PASS_EXEC_AP` — serial has `exec-ap ran_on=core2` (the routed app ran on a
ComputeApp core) AND `EXEC_AP_OK` (wtecho's output reached the terminal). 
- `ran_on=core0` ⇒ it ran inline on the BSP (routing didn't fire — check
  first_compute_app_core / the .cwasm branch). 
- no `EXEC_AP_OK` ⇒ the app's stdout didn't reach the PTY cross-core, or it crashed.
- `#DF` ⇒ AP stack too small (bump arena per C2a).
Do NOT mark C2b done unless `ran_on=core2` + `EXEC_AP_OK`.
ALSO: `make test-boot` (1 core) → `TEST_BOOT_PASS` (inline fallback). `make run-test`
(1 core, .wasm tools on BSP) → `TEST_PASS`. `make run-ssh-gui-test` → PASS (compositor
hand-off unaffected — it's a separate branch). `make run-smp-test`/`run-smp2-test` → PASS.

- [ ] **Step 4: changelog + commit** —
```
git add user-bin/exec-ap-init.sh Makefile CHANGELOG/NN-...
git commit -m "test(smp): C2b — a .cwasm exec runs on a ComputeApp core (run-exec-ap-test)"
```
Trailer as above.

---

## Self-Review
- **App off the BSP:** the routed `.cwasm` runs on core 2; the BSP exec_worker `.await`s
  (async) so the BSP executor keeps polling net/ssh/usb — responsive during the app. The
  completion comes back via the cross-core wake (Step 2, proven in 3c). Gate proves the
  routing (`ran_on=core2`) + the app's stdout reaching the terminal (`EXEC_AP_OK`).
- **Stack kept low:** VFS read + proc register on the BSP; the AP task only runs run_cwasm
  (≈ C2a's profile, fit 65536). Watch #DF; bump arena if needed.
- **Single-slot preserved:** one routed exec at a time (matches EXEC_QUEUE). C2c makes it
  per-core/parallel — the real throughput. Safe + correct here, just not yet parallel.
- **Compositor + wasmi unaffected:** compositor keeps its Step-5 hand-off; `.wasm` (wasmi)
  stays on the BSP (its own runtime; route it later if wanted).
- **Risk:** the borrow restructuring in exec_worker (move bytes to the task vs &bytes
  inline) — get it right (compute the core first, then branch). And the AP stack for the
  real app path. Do NOT mark done unless the gate (`ran_on=core2` + `EXEC_AP_OK`) passes.
