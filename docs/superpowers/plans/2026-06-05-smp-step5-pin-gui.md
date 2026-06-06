# Step 5 — Pin GUI to its own core (THE GOAL) Implementation Plan

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development / executing-plans. `- [ ]`.

**THE GOAL.** Run the compositor on a dedicated GUI core so the BSP executor stays alive
for I/O — **GUI fluid WHILE SSH/net/USB stay responsive** (today the compositor runs
inline on the BSP executor and never returns → SSH/net/USB die when the GUI starts).
Spec §10. Builds on the foundation: 1a/1b/Step2/3a/3b/3c (per-core executors, message
bus, cross-core wake+spawn — all committed + verified).

**Approach (spec §10, surgical pin):** a `CORE_ROLES` table assigns one AP the GUI role;
that AP runs `gui_worker_loop` (waits for the cwasm, then `run_compositor_gate` — a
`-> !` busy-spin loop, NOT an embassy task) instead of `run_core`. `exec_worker_task` on
the BSP, on the `compositor.cwasm` branch, HANDS OFF the bytes to the GUI core's mailbox
+ wakes it + RETURNS (freeing the BSP executor) instead of running the gate inline. USB
polling de-dup: the BSP's `usb_poll_task` now runs (executor alive), so remove the
`usb::poll()` band-aid from `fold_mouse`. 1-core fallback: gate inline on the BSP (today).

**Why the GUI core is a spinner, not an executor:** `run_compositor_gate` is a `-> !`
loop that owns its core (busy-spins for pacing, uses gfx queues — NOT the executor's
Delay). It does not fit embassy's async task model. So the GUI core is dedicated to it
(like the old `ap_worker_loop` was dedicated to the pool). Other APs run `run_core` (3b)
and drain the compute pool → banded compositing still has workers.

**Prerequisites (committed):** Step 2 (`wake_core` + targeted IPI), 3b (`run_core` for
the other APs — they drain the pool for compositing bands), 3c (cross-core spawn — not
strictly needed here but the wake infra it proved underpins the mailbox wake).

**3d (TLB shootdown) deferred:** the compositor is fully instantiated on the BSP before
hand-off; on the GUI core it renders (heap/magazine allocs, no shared-MAPPER mutation at
steady state) → no stale-TLB exposure for THIS pin. 3d is needed later for dynamic
WASM-app cores. Documented, not blocking here.

---

## File Structure
- `kernel/src/cpu/mod.rs` (or `smp/mod.rs`) — `enum CoreRole`, `CORE_ROLES[MAX_CPUS]`,
  `set_core_role`/`core_role`.
- `kernel/src/smp/mod.rs` — `bringup()` assigns roles (BSP=0 BspIo; first AP=GuiCompositor
  if it exists; rest=ComputeApp).
- `kernel/src/cpu/ap.rs` — `ap_entry` branches on `core_role(cpu)`: GuiCompositor →
  `gui_worker_loop()`; else → `executor::run_core(cpu)`.
- `kernel/src/wasm/wt/wm.rs` (or a small `gui` module) — `CompositorMailbox` +
  `gui_worker_loop()` + `send_compositor_to_gui_core(bytes) -> bool`.
- `kernel/src/executor/mod.rs` — `exec_worker_task` compositor branch: hand off, else
  inline (1-core fallback).
- `kernel/src/gfx/mod.rs` — remove `usb::poll()` from `fold_mouse`.
- `tests/ssh-during-gui-test.sh` (new) + `Makefile` target `run-ssh-gui-test`.
- `CHANGELOG/NN`.

---

## Task 1: CORE_ROLES table + role-based AP dispatch

**Files:** `kernel/src/cpu/mod.rs`, `kernel/src/smp/mod.rs`, `kernel/src/cpu/ap.rs`

- [ ] **Step 1: CoreRole + table** — In `cpu/mod.rs`:
```rust
#[repr(u8)]
#[derive(Clone, Copy, PartialEq)]
pub enum CoreRole { BspIo = 0, GuiCompositor = 1, ComputeApp = 2 }
static CORE_ROLES: [AtomicU8; MAX_CPUS] = { const Z: AtomicU8 = AtomicU8::new(2); [Z; MAX_CPUS] }; // default ComputeApp
pub fn set_core_role(cpu: u32, role: CoreRole) { CORE_ROLES[cpu as usize].store(role as u8, Ordering::SeqCst); }
pub fn core_role(cpu: u32) -> CoreRole {
    match CORE_ROLES[cpu as usize].load(Ordering::SeqCst) { 0 => CoreRole::BspIo, 1 => CoreRole::GuiCompositor, _ => CoreRole::ComputeApp }
}
```

- [ ] **Step 2: assign roles in bringup** — In `smp/mod.rs bringup()`: set BSP role +
  pick the first started AP as the GUI core. After `set_cpu_mapping(bsp_lapic, 0)`:
  `crate::cpu::set_core_role(0, CoreRole::BspIo);`. In the AP loop, the FIRST AP (id==1)
  gets `set_core_role(1, CoreRole::GuiCompositor)`, the rest stay default ComputeApp.
  (set the role BEFORE `cpu.bootstrap(...)` so the AP reads it in ap_entry.) Log the table.

- [ ] **Step 3: ap_entry role dispatch** — In `cpu/ap.rs ap_entry`, replace the final
  `crate::executor::run_core(cpu_id as u32)` (3b) with:
```rust
    match crate::cpu::core_role(cpu_id as u32) {
        crate::cpu::CoreRole::GuiCompositor => crate::wasm::wt::wm::gui_worker_loop(),
        _ => crate::executor::run_core(cpu_id as u32),
    }
```
(Keep the `set_tsc_aux`/`start_ap_timer`/`mark_online` sequence before this.)

- [ ] **Step 4: build** — `make test-boot` (1 core: no AP, BSP=run_core(0), no GUI core)
  → `TEST_BOOT_PASS`. (`gui_worker_loop` added in Task 2 — do Task 2 before building.)

---

## Task 2: Compositor mailbox + gui_worker_loop + hand-off

**Files:** `kernel/src/wasm/wt/wm.rs`, `kernel/src/executor/mod.rs`

- [ ] **Step 1: CompositorMailbox + gui_worker_loop in wm.rs** —
```rust
use core::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
struct CompositorMailbox { ptr: AtomicUsize, len: AtomicUsize, ready: AtomicBool }
static COMPOSITOR_MAILBOX: CompositorMailbox =
    CompositorMailbox { ptr: AtomicUsize::new(0), len: AtomicUsize::new(0), ready: AtomicBool::new(false) };

/// GUI-core entry: wait (halt) for the BSP to hand off the compositor cwasm, then run
/// it forever on THIS core. The BSP executor is meanwhile free to poll I/O.
pub fn gui_worker_loop() -> ! {
    crate::binfo!("wm", "gui core {} waiting for compositor", crate::cpu::cpu_id());
    loop {
        if COMPOSITOR_MAILBOX.ready.load(Ordering::Acquire) {
            let ptr = COMPOSITOR_MAILBOX.ptr.load(Ordering::Acquire) as *const u8;
            let len = COMPOSITOR_MAILBOX.len.load(Ordering::Acquire);
            // SAFETY: the BSP leaked a 'static [u8] and published ptr/len with Release
            // before setting ready; we Acquire-load. The compositor never returns, so
            // the leak lives forever.
            let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
            run_compositor_gate(bytes); // -> !
        }
        // Nothing yet: halt until the hand-off IPI (wake_core) wakes us.
        x86_64::instructions::interrupts::disable();
        if !COMPOSITOR_MAILBOX.ready.load(Ordering::Acquire) {
            x86_64::instructions::interrupts::enable_and_hlt();
        } else {
            x86_64::instructions::interrupts::enable();
        }
    }
}

/// BSP: hand the compositor cwasm to the GUI core (if one exists). Returns true if
/// handed off (caller must NOT run the gate inline), false if no GUI core (1-core
/// fallback → caller runs the gate inline). `bytes` MUST outlive the kernel (leaked).
pub fn send_compositor_to_gui_core(bytes: &'static [u8]) -> bool {
    // Find the GUI core (role GuiCompositor, online).
    let mut gui = None;
    for c in 0..crate::cpu::cpus_online() {
        if crate::cpu::core_role(c) == crate::cpu::CoreRole::GuiCompositor { gui = Some(c); break; }
    }
    let gui = match gui { Some(c) => c, None => return false };
    COMPOSITOR_MAILBOX.ptr.store(bytes.as_ptr() as usize, Ordering::Release);
    COMPOSITOR_MAILBOX.len.store(bytes.len(), Ordering::Release);
    COMPOSITOR_MAILBOX.ready.store(true, Ordering::Release);   // publish AFTER ptr/len
    crate::executor::wake_core(gui);                            // targeted IPI → GUI core leaves hlt
    crate::binfo!("wm", "compositor handed off to gui core {}", gui);
    true
}
```

- [ ] **Step 2: exec_worker_task hand-off** — In `executor/mod.rs exec_worker_task`, the
  `compositor.cwasm` branch (currently `crate::wasm::wt::wm::run_compositor_gate(&bytes);`
  inline). Replace with: leak the bytes (so the GUI core can use them 'static), try the
  hand-off; only run inline if no GUI core:
```rust
    if slot.path.ends_with("compositor.cwasm") {
        // Leak the bytes: the compositor never returns, so the 'static is correct, and
        // the GUI core needs them for the kernel's lifetime.
        let leaked: &'static [u8] = alloc::boxed::Box::leak(bytes.into_boxed_slice());
        if crate::wasm::wt::wm::send_compositor_to_gui_core(leaked) {
            // Handed off to the GUI core; this exec task returns so the BSP executor
            // keeps polling I/O (net/usb/ssh). Report a pid + done like a normal exec.
            EXEC_QUEUE.result.store(0, Ordering::SeqCst);
            EXEC_QUEUE.done.store(true, Ordering::SeqCst);
            if let Some(w) = EXEC_QUEUE.shell_waker.lock().take() { w.wake(); }
            continue;
        }
        // No GUI core (1 core): fall back to running it inline (today's path).
        crate::wasm::wt::wm::run_compositor_gate(leaked);
    }
```
(Match the surrounding code: `bytes` is the `Vec<u8>` read from VFS — confirm
`.into_boxed_slice()` works; adjust to the actual local var name/type. The
`EXEC_QUEUE`/done/waker handling must mirror how a normal exec completes so the boot
shell that ran `compositor` doesn't hang waiting for a result.)

- [ ] **Step 3: `wake_core` is pub** — confirm `executor::wake_core` (Step 2) is `pub`
  (it is). It sets `WAKE_PENDING[gui]` + targeted `VEC_WAKE` IPI. The GUI core's
  `gui_worker_loop` halt is woken by that IPI (VEC_WAKE handler = eoi → leaves hlt →
  re-checks `ready`).

- [ ] **Step 4: build** — `make test-boot` (1 core) → `TEST_BOOT_PASS`. On 1 core
  `send_compositor_to_gui_core` returns false (no GUI core) → inline fallback = today's
  behavior. If the shell that execs `compositor` is used in test-boot, it still works.

- [ ] **Step 5: commit** (Tasks 1+2 together) —
```
git add kernel/src/cpu/mod.rs kernel/src/smp/mod.rs kernel/src/cpu/ap.rs kernel/src/wasm/wt/wm.rs kernel/src/executor/mod.rs
git commit -m "feat(smp): Step 5 — pin GUI to a dedicated core; BSP hands off compositor (Step 5 part 1)"
```
Trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## Task 3: USB poll de-dup

**Files:** `kernel/src/gfx/mod.rs`

- [ ] **Step 1: remove the band-aid** — In `fold_mouse` (gfx/mod.rs ~:234), DELETE the
  `crate::usb::poll();` line. Update the doc comment: the sync GUI no longer starves
  `usb_poll_task` because the compositor runs on its own core while the BSP executor
  keeps polling USB. (On the 1-core fallback the GUI is inline again and USB would starve
  — but that's the degraded path; if you want, keep the `usb::poll()` ONLY under a
  `core_role(cpu_id())==GuiCompositor`-is-absent check. Simplest: remove it; 1-core is a
  fallback, not the target.)

- [ ] **Step 2: build + run -smp 4** — `make iso CARGO_FEATURES="boot-checks"` + boot
  `-smp 4`; confirm boot OK + (if the GUI launches in this init) the compositor marker.
  `make test-boot` (1 core) → `TEST_BOOT_PASS`.

- [ ] **Step 3: commit** —
```
git add kernel/src/gfx/mod.rs
git commit -m "feat(smp): Step 5 — drop usb::poll band-aid from fold_mouse (BSP owns USB now)"
```
Trailer as above.

---

## Task 4: THE GOAL GATE — SSH alive while the GUI runs

**Files:** `tests/ssh-during-gui-test.sh` (new), `Makefile`, `CHANGELOG/NN`

- [ ] **Step 1: the goal test script** — Create `tests/ssh-during-gui-test.sh` by adapting
  `tests/ssh-shell-test.sh` with TWO changes: (a) the ISO is built with
  `INIT=user-bin/compositor-init.sh` (so the boot launches the compositor → exercises the
  hand-off); (b) QEMU gets `-smp 4` (so a GUI core exists). Keep the rest (port-forward,
  ssh exec `pwd` + interactive, the auth-ok + `ruos:/$` prompt assertions). The script:
  - builds the ISO: `make iso INIT_SCRIPT=user-bin/compositor-init.sh` (+ ssh-key-on-disk).
  - boots: `qemu ... -smp 4 ... -netdev user,hostfwd=tcp:127.0.0.1:2222-:22 -device virtio-net-pci,...`.
  - waits for the GUI to come up (grep serial for the compositor hand-off marker
    `compositor handed off to gui core` — proves the pin happened), THEN runs ssh.
  - asserts: `auth ok` in serial AND interactive prompt `ruos:/$` received.
  - **This is the goal proof:** SSH auth + a working shell prompt while the compositor
    runs ⇒ the BSP executor (ssh_serve_task) stayed alive during the GUI. Before Step 5
    this FAILS (executor blocked by the inline gate).

- [ ] **Step 2: Makefile target** — Add:
```makefile
.PHONY: run-ssh-gui-test
run-ssh-gui-test: ssh-key-on-disk
	@$(MAKE) iso INIT_SCRIPT=user-bin/compositor-init.sh
	bash tests/ssh-during-gui-test.sh
```

- [ ] **Step 3: run the goal gate** —
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-ssh-gui-test'
```
GATE: prints `TEST_PASS_SSH` AND the serial contains `compositor handed off to gui core N`
(the pin happened) AND `auth ok` + the client got `ruos:/$`. This is THE GOAL achieved:
GUI on its core, SSH alive. If SSH fails (no auth / no prompt) while the compositor
marker IS present, the pin didn't free the BSP executor → debug the hand-off (did
exec_worker return? did the GUI core pick up the mailbox?). Do NOT claim the goal on a
failing SSH.

- [ ] **Step 4: regression — GUI still renders pinned** — `make run-comp-smp-test` (SP4
  banded-composite equivalence; the GUI now runs on its core, bands on the compute APs).
  Must still pass (byte-identical composite + ≥2 composite cores). Also `make test-boot`
  + `make run-test` + `make run-smp-test`/`run-smp2-test`.

- [ ] **Step 5: changelog + commit** — next free number. Cosa: Step 5 — GUI pinned to a
  dedicated core; SSH-during-GUI test proves I/O stays alive. THE GOAL. Perché: the
  compositor no longer blocks the BSP executor; GUI + I/O run in parallel.
```
git add tests/ssh-during-gui-test.sh Makefile CHANGELOG/NN-...
git commit -m "test(smp): Step 5 GOAL — SSH alive while GUI runs on its own core"
```
Trailer as above.

---

## Self-Review
- **THE GOAL is the gate:** `run-ssh-gui-test` = SSH works while the compositor runs ⇒
  the BSP executor (ssh_serve_task/net/usb) was NOT blocked by the GUI. This is the exact
  symptom that was broken; the test directly proves it fixed.
- **GUI core is a dedicated spinner** (`gui_worker_loop` → `run_compositor_gate`, `-> !`),
  not an executor — correct, since the gate busy-spins and uses gfx queues, not Delay.
- **Banded compositing preserved:** other APs run `run_core` (3b) which drains the pool;
  the GUI core submits bands to the pool → run-comp-smp-test is the gate.
- **Cross-core safety:** the hand-off mailbox uses Release(ptr/len)→Release(ready) /
  Acquire on the GUI core + a `wake_core` IPI (Step 2). gfx input queues are IrqMutex
  (cross-core safe); the GUI core owns the back-buffer; GFX_* geometry is read-mostly.
- **USB:** BSP `usb_poll_task` polls USB (executor alive); `fold_mouse` just drains the
  populated queues. 1-core fallback degrades (USB starves under inline GUI) — acceptable.
- **3d (TLB) deferred:** compositor instantiated on BSP before hand-off; no shared-MAPPER
  mutation on the GUI core at steady state. Flagged for dynamic WASM-app cores later.
- **Risk:** the exec_worker hand-off must complete the EXEC_QUEUE handshake (done+waker)
  so the boot shell that ran `compositor` doesn't hang. And the GUI core must reliably
  wake from its `hlt` on the hand-off IPI (Step 2's wake_core, proven in 3c). If the GUI
  is blank or SSH hangs, check those two. Do NOT mark the GOAL done unless run-ssh-gui-test
  PASSES and run-comp-smp-test still renders.
