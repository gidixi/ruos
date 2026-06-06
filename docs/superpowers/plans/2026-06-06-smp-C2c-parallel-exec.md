# C2c — Parallel exec (per-request, multiple apps on multiple cores) Implementation Plan

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development / executing-plans. `- [ ]`.
> ⚠️ HIGH-RISK: touches the exec path used by EVERY tool. A bug here breaks all exec →
> `run-test` fails. Implement carefully + keep `run-test` green at every commit.

**Goal:** multiple `.cwasm` apps run on multiple ComputeApp cores CONCURRENTLY — the full
general-throughput win. Today `EXEC_QUEUE` is single-slot and ASSUMES sequential exec
(two shells exec'ing concurrently would corrupt the shared `pending` slot — a latent bug
C2c also fixes). Spec §3.5. Builds on C2b (single `.cwasm` exec routed to a core, proven).

**Design (per-request, lower blast radius than ripping out EXEC_QUEUE):**
- Split at the exec entry (`fiber.rs:~430` / `ruos_exec`): a `.cwasm` regular app takes a
  NEW **per-request parallel path**; `.wasm` (wasmi) keeps the existing single-slot
  `EXEC_QUEUE` (unchanged — wasmi stays on the BSP; lower risk). The compositor keeps its
  Step-5 hand-off (checked first).
- Per-request parallel path: read bytes (async, on the calling fiber — no big stack, the
  run is on the AP), `proc::register`, pick a least-loaded ComputeApp core, `spawn_on`
  `run_app_on_core(bytes, argv, pts, reply: Arc<ExecReply>)` (pool_size = N), `await` the
  per-request reply Arc, `proc::unregister`, return code. Each exec has its OWN reply Arc
  → no shared-slot corruption → concurrent execs are safe + parallel.
- `run_app_on_core`: bump `pool_size` from 1 to N (e.g. MAX_CPUS-2 or 4) + take the reply
  Arc as a moved arg (replacing the single static `APP_REPLY` from C2b).
- Core selection: `pick_compute_core()` = least-loaded ComputeApp core via `cpustat`
  busy/idle, or round-robin (simpler; start round-robin). Returns None on 1-2 core → inline.

**Prerequisites (committed):** C2b (route single .cwasm to a core; `run_app_on_core`,
`first_compute_app_core`), 3c (spawn_on), Step 2 (cross-core wake). C1/C2a (runtime on AP).

**CHANGELOG:** next free on this branch. Trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## The testing challenge (READ — design the gate before coding)

Proving "2 apps on 2 cores at once" needs TWO concurrent CPU-heavy `.cwasm` runs that
OVERLAP in time. Problems:
- Real tools are mostly `.wasm` (wasmi), not `.cwasm`. The only `.cwasm` tool is
  `wtecho.cwasm` — too fast to observe overlap.
- A boot-check can't easily issue 2 concurrent shell execs (no shells in the boot phase).

**Recommended gate (boot-check, direct):** add a CPU-heavy `.cwasm` (a small spin-loop
guest, e.g. a new `tools/wt-spin/` precompiled to `kernel/src/wasm/wt/spin.cwasm` via the
existing `wt-precompile`, that busy-loops ~N million times then returns), then a
boot-check that: spawns TWO `run_app_on_core(spin.cwasm, ...)` with per-request replies
onto cores 2 AND 3 simultaneously, records `t0`, awaits BOTH, records `t1`. Assert: both
`ran_on` are DISTINCT (core2 + core3) AND `(t1-t0) ≈ single-run-time` (overlap, not 2×).
Marker: `parallel-exec cores=[2,3] wall_ms=X single_ms=Y overlap=true`.
- `overlap=true` (wall ≈ single, not 2×) ⇒ the two apps ran in PARALLEL on 2 cores = the
  throughput win. THE PROOF.
- Alternatively/additionally: `run-exec-ap-test` extended to 2 concurrent SSH `wtecho`
  sessions, asserting both complete + cpustat shows cores 2 AND 3 accumulated busy — but
  the boot-check overlap test is more decisive + deterministic.

(Building a spin.cwasm: mirror `tools/wt-reactor/` — a no_std wasm32-unknown-unknown guest
with a `run` export that spins; add a Makefile rule like `reactor.cwasm`'s. ~30 min.)

---

## Task 1: per-request ExecReply + pick_compute_core + pool_size

**Files:** `kernel/src/executor/mod.rs`, `kernel/src/cpu/mod.rs` (or `sched/cpustat.rs`)

- [ ] **Step 1: per-request reply** — Replace the single static `APP_REPLY` (C2b) with a
  per-request `Arc<ExecReply>` (`{ code: AtomicI32, done: AtomicBool, waker: IrqMutex<Option<Waker>> }`),
  created per exec. `run_app_on_core` takes `reply: alloc::sync::Arc<ExecReply>` as a moved
  arg (Arc is Send) + calls `reply.complete(code)`. The awaiting future polls that Arc.
  (The Arc crosses cores — allocated on the BSP, dropped on the AP + BSP; the magazine
  handles the cross-core free, Step 1b.)
- [ ] **Step 2: pool_size** — `#[embassy_executor::task(pool_size = N)]` on `run_app_on_core`
  (N = e.g. `MAX_CPUS - 2` or a fixed 4 — enough concurrent app runs). Verify the embassy
  task arena (65536) holds N concurrent run_cwasm tasks; bump if needed.
- [ ] **Step 3: pick_compute_core** — round-robin (an `AtomicUsize` cursor) over the
  online ComputeApp cores (or least-loaded via cpustat busy/idle). `None` on 1-2 core.
- [ ] **Step 4: build** — `make test-boot` → `TEST_BOOT_PASS`.

## Task 2: per-request parallel .cwasm exec path

**Files:** `kernel/src/wasm/fiber.rs` (the exec dispatch ~430), `kernel/src/executor/mod.rs`, maybe `kernel/src/wasm/exec_queue.rs`

- [ ] **Step 1: split the exec entry** — In `fiber.rs` (the `post_and_wait` call site for
  exec): if `path.ends_with(".cwasm")` AND not the compositor → call a NEW
  `exec_cwasm_parallel(path, argv, cwd, pts).await`; else → the existing
  `EXEC_QUEUE.post_and_wait(...)` (unchanged, .wasm/wasmi + compositor).
- [ ] **Step 2: exec_cwasm_parallel** — reads bytes (async), `proc::register`,
  `pick_compute_core()`: Some(core) → `Arc<ExecReply>`, `spawn_on(core,
  run_app_on_core(bytes.into_boxed_slice(), argv, pts, reply.clone()))`, `reply.wait().await`;
  None → inline `run_cwasm`. `proc::unregister`. Return code. (This is the C2b logic made
  per-request + concurrent.)
- [ ] **Step 3: keep C2b's single-path or remove it** — the C2b exec_worker `.cwasm`
  routing is now superseded by exec_cwasm_parallel at the fiber level. Decide: route at
  the fiber (Step 1) and let exec_worker handle ONLY .wasm + compositor; OR keep
  exec_worker routing .cwasm and make IT per-request. Cleanest: route .cwasm at the fiber
  (exec_cwasm_parallel), exec_worker handles the rest. Ensure no double-handling.
- [ ] **Step 4: build + run-test (CRITICAL regression)** — `make run-test` (1 core) →
  `TEST_PASS`. Exec is used by every tool; a break here fails run-test. Also `make
  run-exec-ap-test` → still `TEST_PASS_EXEC_AP` (single .cwasm still routes). `make
  run-ssh-gui-test` → PASS.
- [ ] **Step 5: commit** —
```
git commit -m "feat(smp): C2c — per-request parallel .cwasm exec (each exec its own reply + core)"
```

## Task 3: spin.cwasm + the parallelism gate

**Files:** `tools/wt-spin/*` (new), `Makefile`, `kernel/src/wasm/wt/*` (embed), `kernel/src/boot/phases/interrupts.rs`, `CHANGELOG/NN`

- [ ] **Step 1: spin.cwasm guest** — mirror `tools/wt-reactor/`: a no_std
  wasm32-unknown-unknown guest with a `run` export that busy-loops ~50M iters + returns.
  Makefile rule → `kernel/src/wasm/wt/spin.cwasm` (like `reactor.cwasm`). Embed via
  `include_bytes!` under boot-checks. Add a `run_spin_on_core`-style runner.
- [ ] **Step 2: parallelism boot-check** — on -smp 4 (cores 2,3 ComputeApp): spawn 2
  `run_app_on_core(spin.cwasm,...)` onto cores 2 and 3 with per-request replies, time the
  concurrent await vs a single run, log `parallel-exec cores=[..] wall_ms=.. single_ms=.. overlap=..`.
- [ ] **Step 3: gate -smp 4** — `overlap=true` (wall ≈ single, distinct cores 2+3) ⇒
  2 apps ran in parallel on 2 cores = THE throughput proof. + test-boot/run-test/
  run-smp/smp2/ssh-gui/exec-ap all PASS.
- [ ] **Step 4: changelog + commit.**

---

## Self-Review / risks
- **Blast radius:** the exec path runs EVERY tool. Keep `.wasm`/wasmi on the unchanged
  single-slot EXEC_QUEUE; only `.cwasm` takes the new per-request path. `run-test` (lots
  of tool execs) is the regression gate at EVERY commit.
- **Concurrent-exec correctness:** per-request Arc reply (no shared single slot) fixes the
  latent bug (2 shells exec'ing would corrupt the single EXEC_QUEUE.pending today).
- **Stack:** N concurrent run_app_on_core (pool_size N) each run run_cwasm on its core's
  poll stack (C2a/C2b fit 65536 for ONE; N concurrent are on N DIFFERENT cores' stacks, so
  per-core it's still one — fine). The embassy ARENA holds N task states (bump if needed).
- **Test:** needs a CPU-heavy `.cwasm` (spin.cwasm) to observe overlap — the main extra
  work. The boot-check overlap measure (wall ≈ single) is the decisive parallelism proof.
- **NOTE:** this is the HIGHEST-RISK + most design-heavy remaining step. Treat this plan as
  a strong starting design but VERIFY it against the code (esp. the fiber exec dispatch +
  whether routing belongs at the fiber or exec_worker level) and report design issues
  before committing the rework.
