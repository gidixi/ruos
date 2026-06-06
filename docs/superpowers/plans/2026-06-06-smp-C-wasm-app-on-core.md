# C — WASM apps on ComputeApp cores Implementation Plan

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development / executing-plans. `- [ ]`.

**Goal:** run WASM app instances on the `ComputeApp` cores (not just the BSP) → general
SMP throughput beyond the GUI (multiple apps in parallel on different cores). Spec §3.5
(WASM-app cores). Decomposed:
- **C1 (de-risk, THIS plan, detailed):** prove the WASM runtime (wasmtime AOT) runs an
  instance CORRECTLY on a non-BSP core. The risky unknown — if the runtime has hidden
  single-core state or the embassy task stack is too small, C2 is moot.
- **C2 (the throughput win, OUTLINED):** route `exec`'d apps to ComputeApp cores +
  multiple in parallel. Mostly plumbing once C1 proves the runtime works off-BSP.

**Prerequisites (committed):** 3b (per-core executors), 3c (`spawn_on`), 3d (TLB
shootdown — the instance loads code/grows memory → MAPPER mutations → shootdown), 1b
(magazine per-core alloc), 1a (fast cpu_id). The global subsystem locks (NET/CONSOLE/
PTY/REGISTRY) are SMP-safe (Step 7 audit) → an app on core 2 using them is safe-but-
contended (Step 4 later reduces the contention; NOT a correctness blocker for C).

**CHANGELOG:** next free on this branch. Trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## C1 — Prove the WASM runtime runs on a ComputeApp core (de-risk)

**Files:** `kernel/src/executor/mod.rs` (probe task + result statics), `kernel/src/boot/phases/interrupts.rs` (boot-check), `CHANGELOG/NN`.

The risky unknowns:
1. **Runtime global state:** `wasm/wt` ENGINE is read-mostly after init (spec §2.2); each
   instance has its own store/linear-memory/fuel. Running an instance on core 2 should be
   safe — but VERIFY (no hidden single-core mutable global in the wasmtime no_std glue).
2. **Stack:** `run_hello_demo()` instantiates+runs a cwasm (stack-heavy). Today it runs on
   the boot stack (BSP boot-check). Inside a `spawn_on`'d embassy task on core 2, the
   stack is the task-arena slice (`task-arena-size-65536`). If it overflows → crash/#DF.
   The existing `exec_worker_task` has its own embassy stack frame for exactly this reason.
3. **TLB:** instantiation maps code (W^X via set_flags) + linear memory → on core 2 these
   now fire TLB shootdowns (3d). Confirm no hang.

- [ ] **Step 1: probe task + result statics** — In `executor/mod.rs`, under boot-checks:
```rust
#[cfg(feature = "boot-checks")]
pub static WASM_AP_RAN_ON: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(u32::MAX);
#[cfg(feature = "boot-checks")]
pub static WASM_AP_OK: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(2); // 0=fail 1=ok 2=unset

#[cfg(feature = "boot-checks")]
#[embassy_executor::task]
async fn wasm_ap_probe() {
    // Runs the embedded hello.cwasm (wasmtime AOT) on whatever core this task is on.
    // Proves the runtime instantiates + executes correctly off the BSP.
    let ok = crate::wasm::wt::run_hello_demo();
    WASM_AP_RAN_ON.store(crate::cpu::cpu_id(), core::sync::atomic::Ordering::SeqCst);
    WASM_AP_OK.store(ok as u32, core::sync::atomic::Ordering::SeqCst);
}
```
> If the embassy task stack is too small for `run_hello_demo` (watch for a #DF / crash in
> the gate), options: (a) bump `task-arena-size-65536` → larger in `kernel/Cargo.toml`
> (it's the embassy arena); (b) give the probe its own larger stack like
> `exec_worker_task` does. Try the plain task first; escalate only if it crashes.

- [ ] **Step 2: boot-check** — In `interrupts.rs` under boot-checks, after bringup, on a
  ComputeApp core (core 2 on SMP≥3):
```rust
    #[cfg(feature = "boot-checks")]
    if crate::cpu::cpus_online() >= 3 && crate::cpu::core_role(2) == crate::cpu::CoreRole::ComputeApp {
        let mut spawned = false;
        for _ in 0..1_000_000u64 { if crate::executor::spawn_on(2, crate::executor::wasm_ap_probe()).is_ok() { spawned = true; break; } core::hint::spin_loop(); }
        let mut ok = 2u32; let mut ran = u32::MAX;
        for _ in 0..200_000_000u64 {
            let o = crate::executor::WASM_AP_OK.load(core::sync::atomic::Ordering::SeqCst);
            if o != 2 { ok = o; ran = crate::executor::WASM_AP_RAN_ON.load(core::sync::atomic::Ordering::SeqCst); break; }
            core::hint::spin_loop();
        }
        crate::binfo!("wasm-ap", "ran_on=core{} ok={} spawned={} (expect core2 ok=1)", ran, ok, spawned);
    }
```
(`wasm_ap_probe`/`WASM_AP_*`/`spawn_on` must be `pub`. The probe future must be Send —
`run_hello_demo` captures nothing non-Send.)

- [ ] **Step 3: gate — build + boot -smp 4, TWICE** —
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso CARGO_FEATURES="boot-checks" && for i in 1 2; do echo "run $i"; timeout 90 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -m 512 -no-reboot -display none -serial stdio -device qemu-xhci -cdrom build/os.iso 2>&1 | grep -E "wasm-ap|APs online|#DF|#PF|#GP|panic"; done'
```
GATE: `wasm-ap ran_on=core2 ok=1 spawned=true`, stable BOTH runs, NO fault (#DF/#PF/#GP)/
panic. 
- `ran_on=core2 ok=1` ⇒ the wasmtime AOT runtime instantiated + ran the cwasm correctly
  on a ComputeApp core = the runtime works off-BSP = C2 is unblocked. THE PROOF.
- a fault/#DF ⇒ likely stack overflow in the embassy task → bump arena / dedicated stack.
- `ok=0` ⇒ the run failed on core 2 (runtime global-state issue?) → investigate.
- `ran_on=core0` ⇒ it ran on the BSP (spawn routed wrong) → check spawn_on.
ALSO: `make test-boot` (1 core) → `TEST_BOOT_PASS` (skipped on <3 cores). `make
run-smp-test`/`run-smp2-test`/`run-ssh-gui-test` → PASS (no regression).

- [ ] **Step 4: changelog + commit** —
```
git add kernel/src/executor/mod.rs kernel/src/boot/phases/interrupts.rs CHANGELOG/NN-... [kernel/Cargo.toml if arena bumped]
git commit -m "test(smp): C1 — WASM (wasmtime AOT) instance runs correctly on a ComputeApp core"
```
Trailer as above.

---

## C2 — Route exec'd apps to ComputeApp cores (the throughput win, OUTLINE)

Once C1 proves the runtime runs off-BSP, distribute REAL `exec`'d apps:
- `exec_worker_task` (BSP) currently runs the .wasm/.cwasm inline on the BSP. Change: for
  a regular app (NOT the compositor), pick the least-loaded ComputeApp core + `spawn_on`
  the run there; the app's host-fns use the global subsystem locks (safe-contended;
  Step 4 later reduces contention). The completion delivers the exit code back to the
  shell fiber via a cross-core wake (Step 2 `__pender` → BSP, proven in 3c).
- **EXEC_QUEUE is single-slot today** (one exec at a time). For PARALLEL apps it must
  become per-core or multi-slot (each ComputeApp core its own exec channel + completion).
  This is the real plumbing of C2.
- Pinning: a simple least-loaded pick (cpustat busy/idle) or round-robin over ComputeApp
  cores. The compositor stays on the GUI core (Step 5); I/O on the BSP.
- Gate: run two CPU-heavy wasm tools concurrently over two SSH sessions; confirm they run
  on DIFFERENT ComputeApp cores (cpustat shows both busy) + both complete + the BSP shell
  stays responsive. That's general SMP throughput delivered.
- Risk: per-core/multi-slot exec channel design; the per-instance stack (each AP running
  wasm needs the dedicated-stack treatment from C1); proc registry contention (Step 4
  proc-per-core helps — but safe without it). C2 gets its own detailed plan after C1.

---

## Self-Review
- **C1 isolates the risky unknown** (runtime-on-AP) into a minimal boot-check — no
  exec/shell/EXEC_QUEUE integration. If C1 fails (stack/global-state), we learn it cheaply
  before building C2's plumbing.
- **Uses the proven foundation:** spawn_on (3c) puts the task on core 2; the per-core
  executor (3b) polls it; instantiation's MAPPER mutations fire TLB shootdowns (3d); the
  magazine (1b) + fast cpu_id (1a) serve its allocs. C1 is the integration test of the
  whole stack under a real wasm workload.
- **Safe without Step 4:** the app's host-fns hit SMP-safe global locks (Step 7 audit) —
  contended, not unsafe. Step 4 (later) reduces the contention C2 reveals.
- **Risk: MEDIUM-HIGH (first wasm-on-AP).** The stack size is the most likely failure
  (#DF). Do NOT mark C1 done unless `ran_on=core2 ok=1` on both runs with no fault.
