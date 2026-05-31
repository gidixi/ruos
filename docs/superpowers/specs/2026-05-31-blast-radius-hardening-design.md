# Blast-radius hardening — "one bug ≠ total compromise": Design Spec + Plan

## Context

ruos runs everything in ring 0: the SSH server, the TCP/IP stack, crypto, and
every `.wasm` module share one address space and one privilege level. The
isolation guarantee rests entirely on two things being correct:

- the wasmi interpreter being memory-safe, and
- the **host-function boundary** — the hand-written code that copies data
  between guest linear memory and the kernel.

There are ~47 raw `mem.read` / `mem.write` calls scattered across
`kernel/src/wasm/host/*` (fd.rs 11, proc.rs 11, sysinfo.rs 7, lifecycle.rs 6,
service.rs 5, others). Each is a guest-controlled offset/length. A single
missing bound check, a sign error, or an integer overflow in any of them is not
a contained fault — it is arbitrary kernel memory access, i.e. total
compromise. We already saw the class of this bug land in `fd_readdir`: a
negative `buf_len` cast to `usize`. We caught it in review; we will not always.

Additional uncontained-fault vectors found in the tree:

- **No fuel/epoch metering** (fiber.rs:36 builds `Config::default()`; the
  "out of fuel" arm at fiber.rs:156 is stubbed and notes fuel is not
  configured). A `.wasm` with a compute loop that never calls a host fn hangs
  the cooperative executor with no way to reclaim the CPU.
- **Unbounded fd table growth** (fiber.rs:287 and :314 do
  `state.fds.push(...)` with no cap). A module that opens in a loop grows the
  Vec until the kernel heap is exhausted — taking down the whole system, not
  just the task.
- **Single global preopen "/"** — every module can `path_open` anywhere in the
  VFS, including `/mnt/passwd`. A logic bug (or a hostile module) in any tool
  reaches the entire filesystem.
- **Panic = dead box.** main.rs panic handler takes `CONSOLE.lock()` (it even
  comments the deadlock risk) then `hcf()`. A kernel panic is neither reliably
  observable nor recoverable.

This spec makes the common, reachable fault classes non-catastrophic:
host-boundary errors, runaway compute, resource exhaustion, path traversal, and
panics. It shrinks the TCB to one audited choke point and proves it with a
fuzz harness.

## Goals

A buggy or malicious `.wasm`, or a bug in a single host function, must not be
able to:

- read or write kernel memory outside the calling instance's linear memory
  (host-boundary safety);
- hang the kernel via an unbounded compute loop (fuel);
- exhaust kernel resources — fds, linear memory, sockets (per-task limits);
- touch VFS paths outside its granted subtree (capability scoping);
- silently brick the machine on a kernel panic (observable + recoverable panic).

## Non-goals — and why (READ THIS)

This spec deliberately does **not** attempt the following. They are not bugs;
they are the project's identity, and "fixing" them turns ruos into a worse,
redundant Linux. They contradict CLAUDE.md's own "Cosa NON faremo".

- **Ring 3 / per-process MMU isolation.** Would discard the WASM-as-sandbox
  thesis and require per-process page tables + a syscall trampoline. Out.
- **SMP / multi-core.** Requires AP bring-up, per-CPU state, and a full audit of
  every lock in the kernel. Separate multi-month effort, not hardening.
- **JIT.** wasmi is interpreter-only; a JIT means Cranelift (not no_std,
  huge) or writing one. Out of scope for safety work.
- **Linux ABI / ELF userland.** Explicit north-star drop. Run software
  recompiled to WASI, never Linux binaries.

## Honest ceiling

This is **defense in depth in ring 0**, not hardware isolation. It makes the
reachable fault classes non-catastrophic and shrinks + audits the boundary
where escapes actually happen. It cannot make a memory-safety bug inside
wasmi itself, or inside the kernel's own `unsafe` code, non-catastrophic —
only a separate address space + CPU privilege level can, and that is explicitly
out of scope. State this limit in README.md; do not over-claim.

## Components

### 1. `kernel/src/wasm/host/mem.rs` (new) — the single audited accessor

Replace all ad-hoc guest memory access with one choke point:
`guest_read(caller, ptr, len) -> Result<Vec<u8>, errno>` and
`guest_write(caller, ptr, bytes) -> Result<(), errno>` (plus a slice/scalar
helper as convenient).

Rules enforced once, here: `len < 0` → EINVAL (28); `ptr < 0` → EFAULT;
`ptr as u64 + len as u64 > memory.data_size()` → EFAULT; zero-length is OK.
Returns the errno the caller should propagate; never panics, never indexes OOB.

Then migrate every `mem.read` / `mem.write` in `wasm/host/*` to these (~47
sites). Audit checklist:
`grep -rn 'mem.read\|mem.write\|\.read(&caller\|\.write(&mut caller' wasm/host/`
must return only the bodies of mem.rs afterward.

This is the highest-value item: it converts every future host fn into "safe by
construction at the boundary."

### 2. `kernel/src/wasm/fiber.rs` — fuel metering

- At :36, `config.consume_fuel(true)` before `Engine::new`.
- After instantiation, seed a budget: `store.set_fuel(INITIAL_FUEL)?` (tune;
  start generous, e.g. a few hundred million).
- Refuel at the host-call boundary: in the run loop, each time a
  `SuspendReason` is dispatched (i.e. the module made a host call and is about
  to resume), top fuel back up. Effect: I/O-bound modules run indefinitely;
  a pure-compute loop with no host calls burns its budget and is killed.
- Wire the existing out-of-fuel arm (:156): on `OutOfFuel`, log
  `wasm: task killed (fuel exhausted)` and return a non-zero exit code — kill
  the task, never the kernel.

(Confirm the exact wasmi 1.0.9 fuel API names — `consume_fuel`/`set_fuel`/the
`OutOfFuel` error variant — against the vendored crate before coding.)

### 3. `kernel/src/wasm/state.rs` + `fiber.rs` — per-task resource limits

- **Bounded fd table.** Define `const MAX_FDS: usize = 128;`. Replace the
  unbounded `state.fds.push(...)` at fiber.rs:287 and :314 with: find a free
  slot or push only if `fds.len() < MAX_FDS`; otherwise return EMFILE (24).
- **Max linear memory.** wasmi exposes a `ResourceLimiter`; implement it on
  `RuntimeState` (or a sibling) to cap `memory_growing` at a per-task ceiling
  (e.g. 64 MiB) and `table_growing`. Attach via `store.limiter(...)`. Confirm
  the 1.0.9 API.
- **Max sockets.** Cap concurrent `FdEntry::Socket` per task (reuse the same
  MAX or a separate small constant); return EMFILE/ENFILE past it.

### 4. capability-scoped preopens — least privilege per task

Today there is one virtual preopen "/" at fd 3 (fd.rs). Make it a per-task
grant:

- Add `pub root: String` (or `Vec<String>` for multiple preopens) to
  `RuntimeState`, set at task spawn. Shell/init get "/"; spawned tools get a
  narrower grant (e.g. their cwd subtree) when the spawn API supports it.
- In `OpenDir`, `PathOpen`, and every `path_*` handler: after `resolve_cwd`,
  canonicalize the path (collapse `.`/`..`, no symlinks exist so this is
  pure string work) and reject with ENOTCAPABLE (76) / EACCES (2) if the
  result does not start with the task's grant prefix. This defeats `../`
  traversal out of the grant.
- Keep `fd_prestat_dir_name` reporting the grant root so wasi-libc resolves
  relative paths against it.

Phase this: ship the canonicalize-and-check against a single "/" grant first
(no behavior change, but the enforcement code lands and is tested), then narrow
grants for spawned tools as a follow-up once `proc_spawn` exists.

### 5. `kernel/src/main.rs` — survivable panic path

Rewrite the panic handler to never deadlock and to be observable:

- `interrupts::disable()` first (already done).
- Write the panic message to serial via `try_lock` (skip if contended — do
  not block) and append to the klog ring buffer.
- Do not take `CONSOLE.lock()` unconditionally; use `try_lock`.
- Then trigger a controlled reset (triple fault, or 0xCF9 reset, or
  qemu-friendly exit under test) instead of `hcf()` so the box recovers rather
  than bricking. Gate "reset vs halt" behind a config/feature so dev builds can
  still halt-for-inspection if preferred.

### 6. host-boundary fuzz harness (host-runnable, no QEMU)

Under `kernel/` (or a sibling test crate) add `#[cfg(test)]` tests that build a
minimal `wasmi Store<RuntimeState>` with a real linear memory and hammer each
host fn (and `guest_read`/`guest_write`) with adversarial inputs: negative
ptr/len, ptr+len overflowing u64, ranges straddling `data_size()`,
zero-length, huge length. Assert: never panics, never OOB, always a sane
errno. Wire into `cargo test` and CI (Fase B). This is what turns "we think
the boundary is safe" into "the boundary is tested."

## Error handling

Decimal WASI errno used: 2 EACCES, 21 EISDIR/EFAULT, 24 EMFILE,
28 EINVAL, 76 ENOTCAPABLE. Match conventions already in wasm/host/*.

## Testing strategy

- Component 6 fuzz harness on the host (fast, deterministic).
- A `.wasm` smoke that spins a tight `loop {}` → assert the task is killed and
  the kernel keeps serving (a follow-up shell prompt appears). Marker e.g.
  `wasm: task killed (fuel exhausted)`.
- A `.wasm` that opens fds in a loop → assert EMFILE and kernel survives.
- A `.wasm` that tries `path_open("/mnt/passwd")` from a narrowed grant →
  assert ENOTCAPABLE.
- All existing tests (run-test, run-ssh-test, run-passwd-test) still pass.

## Done criteria

- grep for raw mem.read/mem.write in wasm/host/ returns only mem.rs.
- A tight-loop `.wasm` is killed by fuel; the SSH/local shell stays responsive.
- fd / memory / socket limits return errnos; no kernel OOM from a single task.
- Path traversal out of a grant is rejected.
- Kernel panic is printed on serial and the box resets (no silent brick, no
  deadlock).
- Host-boundary fuzz harness is green in CI.
- README.md security section states the honest ceiling (ring-0, wasmi-internal
  bugs still fatal).

## Implementation Plan

Per CLAUDE.md: feature branch first; one `CHANGELOG/NN-26-05-31-<slug>.md`
per task using the next free NN (check CHANGELOG/ — the fd_readdir work
pushed the counter past 181). No commit/push unless asked.

**Task 1 — guest_read/guest_write accessor + migrate all host fns.**
New wasm/host/mem.rs; migrate ~47 sites; the grep audit must come back clean.
This alone closes the fd_readdir negative-buf_len class permanently.

**Task 2 — Fuel metering + kill-on-exhaustion.**
`consume_fuel(true)`, budget, refuel-on-host-call, wire the :156 arm.

**Task 3 — Resource limits (fd cap, ResourceLimiter, socket cap).**
Bound the two fds.push sites; implement/attach the limiter.

**Task 4 — Capability scoping (enforce against "/" grant first).**
Add the grant field, canonicalize + prefix-check in path_*/OpenDir.

**Task 5 — Survivable panic path.**
try_lock serial + klog, controlled reset behind a feature.

**Task 6 — Host-boundary fuzz harness + CI wiring.**
`#[cfg(test)]` adversarial tests; add to cargo test and (if Fase B landed) CI.

## Notes for the implementer

- Build the accessor (Task 1) and migrate before anything else — every other
  task adds host-side code that should use it.
- `Caller::get_export("memory")` → Memory; `memory.data_size(&caller)` (or
  `.size()` in pages × 64 KiB) gives the bound. Confirm the exact wasmi 1.0.9
  signatures against `third_party/`the vendored crate, not from memory.
- The fuel API and ResourceLimiter trait shape changed across wasmi versions —
  check 1.0.9 specifically before writing Tasks 2 and 3.
- Do not regress the fd_readdir/path_open paths just merged; they already do
  guest memory I/O that Task 1 will route through the new accessor.

This is Fase A of the semi-pro roadmap; it is the precondition that lets every
later compatibility/feature task add host fns without widening the attack
surface one unchecked mem.write at a time.
