# C2 — Route exec'd apps to ComputeApp cores Implementation Plan

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development / executing-plans. `- [ ]`.

**Goal:** real general SMP throughput — WASM apps run on the `ComputeApp` cores, the BSP
stays free for I/O, and (ultimately) multiple apps run in parallel. Spec §3.5. Decomposed
(each its own gate; C2c is the full throughput win but needs an EXEC_QUEUE rework):
- **C2a (de-risk, THIS plan detailed):** prove `run_cwasm` (the REAL app path: WASI linker
  + argv + PTY-bound stdout — heavier on stack than C1's `run_hello_demo`) runs correctly
  on a ComputeApp core. C1 proved wasmtime-on-AP; C2a proves the heavier WASI path fits.
- **C2b (route, OUTLINE):** `exec_worker_task` routes a `.cwasm` app run to a ComputeApp
  core + delivers the exit code back. App off the BSP; BSP responsive during the run.
- **C2c (parallelism, OUTLINE):** EXEC_QUEUE is single-slot today → rework to per-core /
  multi-slot so 2+ apps run on 2+ cores concurrently. THE full throughput.

**Prerequisites (committed):** C1 (wasmtime AOT runs on an AP — `ran_on=core2 ok=1`), 3b
(per-core executors), 3c (`spawn_on`), 3d (TLB shootdown for the instance's MAPPER
mutations), 1b (magazine), 1a (fast cpu_id). Global subsystem locks (NET/CONSOLE/PTY/
REGISTRY) are SMP-safe (Step 7) → an app on core 2 using them is safe-but-contended.

**CHANGELOG:** next free on this branch. Trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## C2a — Prove run_cwasm (WASI path) runs on a ComputeApp core

**Files:** `kernel/src/executor/mod.rs` (probe task + statics), `kernel/src/boot/phases/interrupts.rs` (boot-check), `CHANGELOG/NN`.

The delta over C1: `run_hello_demo` used a fresh Engine + a trivial cwasm (light stack).
`run_cwasm` (used by real `exec`) goes through the shared `engine()`, the **WASI Linker**
(argv, fd_write, etc.), a per-instance Store + linear memory, and **fibers** for blocking
host calls. The risk: this is heavier on the AP's executor poll stack — possible #DF if
the AP kernel stack is too small. (If run_cwasm runs its heavy work on a FIBER with its
own stack — check `wasm/fiber.rs` — the poll-stack risk is mitigated.)

- [ ] **Step 0: understand the stack path** — READ `kernel/src/wasm/wt/mod.rs::run_cwasm`
  and `kernel/src/wasm/fiber.rs`. Determine whether run_cwasm's heavy instantiation runs
  on the calling stack or on a fiber with its own stack. This tells you the AP-stack risk.
  Also READ `run_echo_demo()` (mod.rs:42) — it calls run_cwasm on the embedded echo.cwasm
  with argv; it's the perfect ready-made WASI probe.

- [ ] **Step 1: probe task + statics** — In `executor/mod.rs`, under boot-checks:
```rust
#[cfg(feature = "boot-checks")]
pub static CWASM_AP_RAN_ON: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(u32::MAX);
#[cfg(feature = "boot-checks")]
pub static CWASM_AP_CODE: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(i32::MIN); // exit code; MIN=unset

#[cfg(feature = "boot-checks")]
#[embassy_executor::task]
pub async fn cwasm_ap_probe() {
    // run_echo_demo() = run_cwasm(echo.cwasm, argv, pts) — the REAL WASI app path.
    let code = crate::wasm::wt::run_echo_demo();
    CWASM_AP_RAN_ON.store(crate::cpu::cpu_id(), core::sync::atomic::Ordering::SeqCst);
    CWASM_AP_CODE.store(code, core::sync::atomic::Ordering::SeqCst);
}
```

- [ ] **Step 2: boot-check** — In `interrupts.rs` under boot-checks after bringup, on a
  ComputeApp core (core 2 on SMP≥3): `spawn_on(2, cwasm_ap_probe())`, spin up to ~200M for
  `CWASM_AP_CODE != i32::MIN`, then:
```rust
    crate::binfo!("cwasm-ap", "ran_on=core{} code={} (expect core2, echo exit code)", ran, code);
```
(Same inline-poll noop-waker pattern as C1. Use the echo demo's known good exit code —
read `run_echo_demo`'s expected return; C1's sibling boot-check `wt wasmtime WASI echo
exit=N` already logs it, so the expected `code` is that N.)

- [ ] **Step 3: gate — build boot-checks + boot -smp 4, TWICE** —
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso CARGO_FEATURES="boot-checks" && for i in 1 2; do echo "run $i"; timeout 90 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -m 512 -no-reboot -display none -serial stdio -device qemu-xhci -cdrom build/os.iso 2>&1 | grep -E "cwasm-ap|wt .*echo exit|#DF|#PF|#GP|panic"; done'
```
GATE: `cwasm-ap ran_on=core2 code=<N>` matching the BSP echo demo's exit code (the sibling
`wt wasmtime WASI echo exit=<N>` line), stable BOTH runs, NO fault. `ran_on=core2` + the
right code ⇒ the full WASI app path runs correctly on an AP = C2b unblocked. A #DF ⇒ AP
stack too small → bump the AP kernel stack / run on a fiber (note for C2b). 
> ⚠️ Verify the build is `boot-checks` before reading markers — regression runs rebuild
> `build/os.iso` as DEFAULT (no boot-checks) → markers vanish. Always `make iso
> CARGO_FEATURES="boot-checks"` immediately before the boot when checking these.
ALSO: `make test-boot` (1 core) → `TEST_BOOT_PASS`; `make run-smp-test`/`run-smp2-test`/
`run-ssh-gui-test` → PASS.

- [ ] **Step 4: changelog + commit** —
```
git add kernel/src/executor/mod.rs kernel/src/boot/phases/interrupts.rs CHANGELOG/NN-... [Cargo.toml/stack if bumped]
git commit -m "test(smp): C2a — run_cwasm (WASI app path) runs on a ComputeApp core"
```
Trailer as above.

---

## C2b — Route shell exec to a ComputeApp core (OUTLINE)

`exec_worker_task` (BSP) currently runs `run_cwasm` INLINE. Change: for a regular `.cwasm`
app (NOT the compositor — it has its own hand-off), spawn the run on a ComputeApp core via
`spawn_on`, passing (bytes, argv, pts) as the task's moved args + an `Arc` reply slot
(`AtomicI32 code + AtomicBool done + Waker`); the BSP exec_worker `.await`s the reply (so
the BSP executor keeps polling I/O — net/ssh/usb responsive during the app run), then
completes the EXEC_QUEUE handshake as today. The app's stdout (PTY-bound) reaches the SSH
channel via the global-locked PTY (safe cross-core). proc register/is_kill_pending hit the
global REGISTRY lock (safe-contended; Step 4 proc-per-core optimizes later).
- Stack: the spawned task runs run_cwasm → same AP-stack concern as C2a (handle per C2a).
- `.wasm` (wasmi) tools: most user tools are wasmi, a DIFFERENT runtime path. Decide
  whether to route wasmi too (its own stack/global-state risk) or keep wasmi on the BSP
  initially and route only `.cwasm`. Recommend: `.cwasm` first (C1/C2a proved it); wasmi
  later.
- Gate: `exec /bin/<cpu-heavy>.cwasm` over SSH → a serial marker `exec routed to core 2` +
  the routed task logs `ran_on=core2` + the tool's output reaches the SSH client + the BSP
  shell prompt returns (BSP responsive). EXEC_QUEUE stays single-slot here (serialized).

## C2c — Parallelism: per-core / multi-slot exec (OUTLINE)

EXEC_QUEUE is single-slot (one exec at a time globally) — the bottleneck for parallel
apps. Rework: either N exec slots (a small pool) or a per-ComputeApp-core exec channel, so
2+ shells (e.g. 2 SSH sessions) can each have an app running on a different core
simultaneously. Each in-flight exec gets its own reply slot + completion. Pinning: pick the
least-loaded ComputeApp core (cpustat busy/idle) per exec.
- Gate (THE throughput proof): 2 SSH sessions each `exec` a CPU-heavy `.cwasm` → cpustat
  shows TWO ComputeApp cores busy simultaneously + both complete + the BSP stays
  responsive. That is general SMP throughput delivered.
- This is where Step 4 (proc-per-core, pty-core, log-core) starts to matter for contention
  — but C2c is correct (if contended) without it (global locks are SMP-safe).

---

## Self-Review
- **C2a isolates the WASI-stack risk** (the one new unknown over C1) into a boot-check
  using the ready-made `run_echo_demo` (real run_cwasm + WASI). If it #DFs we learn the AP
  needs a bigger stack / fiber before building C2b's routing.
- **C2b/C2c are the real value** (apps off the BSP, then parallel) but C2c needs an
  EXEC_QUEUE rework (single-slot → multi) — a non-trivial design, own plan.
- **Safe without Step 4:** apps on cores use SMP-safe global locks (contended). Step 4
  reduces the contention C2c reveals — not a correctness blocker.
- **Risk:** AP stack for the heavier WASI path (C2a's gate). Do NOT mark C2a done unless
  `ran_on=core2` + correct echo exit code on both runs, no fault.
