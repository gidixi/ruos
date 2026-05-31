# Blast-radius hardening (Fase A) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the common, reachable fault classes in ruos non-catastrophic —
host-boundary memory errors, runaway compute, resource exhaustion, path
traversal, and kernel panics — by shrinking the TCB to one audited choke point
and adding per-task limits, fuel, scoping, and a survivable panic path.

**Architecture:** All guest↔kernel memory access routes through one audited
accessor (`wasm/host/mem.rs`). wasmi fuel metering kills runaway compute;
per-task caps (fd count, linear memory, sockets) stop resource exhaustion; a
canonicalize+prefix-check enforces a per-task VFS grant; the panic handler
becomes non-deadlocking and resets. A host-runnable fuzz harness proves the
boundary.

**Tech Stack:** Rust `no_std`, wasmi 1.0.9 (`Store::set_fuel`/`get_fuel`,
`Config::consume_fuel`, `Memory::{data_size,read,write}`, `ResourceLimiter` +
`Store::limiter`, out-of-fuel detected via `Error::is_out_of_fuel()` /
`TrapCode::OutOfFuel`), `embassy-executor`, `x86_64`.

---

## wasmi 1.0.9 API facts (confirmed against the vendored crate — use these)

- `Memory::data_size(&self, ctx: impl AsContext) -> usize` — the bound.
- `Memory::read(&self, ctx: impl AsContext, offset: usize, buffer: &mut [u8]) -> Result<(), MemoryError>`
- `Memory::write(&self, ctx: impl AsContextMut, offset: usize, buffer: &[u8]) -> Result<(), MemoryError>`
- `Store::set_fuel(&mut self, fuel: u64) -> Result<(), Error>` and `get_fuel(&self) -> Result<u64, Error>`.
- `Config::consume_fuel(&mut self, enable: bool) -> &mut Self`.
- Out-of-fuel: `Error::is_out_of_fuel(&self)` exists (it's `pub(crate)` —
  do NOT call it from kernel; instead match the engine result. Detect via the
  resumable call result variant. See Task 2 for the exact mechanism using the
  existing `ResumableCall`/error path in fiber.rs — the trap is
  `TrapCode::OutOfFuel`, surfaced as an `Error`; compare with
  `err.as_trap_code() == Some(TrapCode::OutOfFuel)`).
- `ResourceLimiter` trait (re-exported at crate root: `wasmi::ResourceLimiter`).
  Methods: `memory_growing(&mut self, current, desired, maximum) -> Result<bool, ...>`,
  `table_growing(&mut self, current, desired, maximum) -> Result<bool, ...>`,
  plus defaulted `instances`/`tables`/`memories`. Attach with
  `Store::limiter(&mut self, impl FnMut(&mut T) -> &mut dyn ResourceLimiter)`.
  Confirm the exact return type (`Result<bool, wasmi::errors::...>` vs older
  `bool`) by reading `~/.cargo/.../wasmi-1.0.9/src/limits.rs` BEFORE coding
  Task 3, and mirror the `impl ResourceLimiter for StoreLimits` there.

Build env: WSL via the **PowerShell tool** (not Bash — git-bash mangles `/mnt`):
`wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | tail -30'`
clean ends `Limine BIOS stages installed successfully.`. Use the **Bash tool**
for git. Host-side `cargo test` (Task 6) also runs in WSL.

CHANGELOG counter: highest existing is **175** → use 176, 177, … per task.
(`ls CHANGELOG/ | grep -oE '^[0-9]+' | sort -n | tail -1` to re-confirm.)

---

## File structure

- Create `kernel/src/wasm/host/mem.rs` — the single audited accessor
  (`guest_read`, `guest_write`, `guest_read_into`, scalar helpers).
- Modify all `kernel/src/wasm/host/*.rs` (43 sites across clock, fd, lifecycle,
  path, proc, random, service, sysinfo, term) → route through mem.rs.
- Modify `kernel/src/wasm/fiber.rs` — fuel config+seed+refuel+kill; bounded
  `fds.push`; attach ResourceLimiter.
- Modify `kernel/src/wasm/state.rs` — `MAX_FDS`/`MAX_SOCKETS` consts, `root`
  grant field, ResourceLimiter impl + mem-cap counter.
- Modify path/dir host fns (`host/path.rs`, `host/fd.rs` OpenDir/path_open) —
  canonicalize + grant prefix check.
- Modify `kernel/src/boot/panic.rs` — non-deadlocking, resetting panic.
- Create `kernel/src/wasm/host/mem_fuzz.rs` (or `#[cfg(test)]` mod in mem.rs) —
  adversarial host-boundary tests.
- Modify `README.md` — honest-ceiling security section.
- One `CHANGELOG/NN-26-05-31-<slug>.md` per task.

---

## Task 1: Single audited guest-memory accessor + migrate all host fns

**Files:**
- Create: `kernel/src/wasm/host/mem.rs`
- Modify: every `kernel/src/wasm/host/*.rs` that calls `mem.read`/`mem.write`
- Modify: `kernel/src/wasm/host/mod.rs` (add `pub mod mem;`)

**FIRST — read to match real signatures:** open `kernel/src/wasm/host/fd.rs`
and `kernel/src/wasm/host/lifecycle.rs` to see the EXACT current pattern: how
`Memory` is obtained (`wasm_memory(&caller)` helper in
`kernel/src/wasm/host/lifecycle.rs` — confirm its name/return type), how
`mem.read(&caller, ptr as usize, &mut buf)` and
`mem.write(&mut caller, ptr as usize, bytes)` are called, and what errno values
each site returns today (e.g. `Error::i32_exit(-1)`, decimal errnos). Your
accessor must drop in without changing the host fns' return semantics.

- [ ] **Step 1: Write the accessor `kernel/src/wasm/host/mem.rs`**

```rust
//! The single audited guest-memory boundary. EVERY host fn that touches guest
//! linear memory goes through here — no raw `mem.read`/`mem.write` elsewhere.
//!
//! One bug in a bound check here is total compromise (ring 0); one correct
//! check here makes every caller safe by construction. Never panics, never
//! indexes out of bounds; returns a WASI errno the caller propagates.

use wasmi::{AsContext, AsContextMut, Caller, Memory};
use alloc::vec::Vec;
use crate::wasm::state::RuntimeState;

/// Decimal WASI errnos used at the boundary.
pub const EINVAL: i32 = 28;
pub const EFAULT: i32 = 21; // ruos convention (matches spec's 21 for EFAULT)

/// Fetch the instance's exported linear memory, or EFAULT if absent.
fn memory(caller: &Caller<'_, RuntimeState>) -> Result<Memory, i32> {
    match caller.get_export("memory") {
        Some(wasmi::Extern::Memory(m)) => Ok(m),
        _ => Err(EFAULT),
    }
}

/// Validate `[ptr, ptr+len)` against the live memory size. Returns the
/// `(usize, usize)` (offset, len) on success. Rejects negative ptr/len and any
/// range that overflows u64 or exceeds `data_size()`. Zero length is allowed.
fn checked_range(
    caller: &Caller<'_, RuntimeState>,
    mem: &Memory,
    ptr: i32,
    len: i32,
) -> Result<(usize, usize), i32> {
    if ptr < 0 || len < 0 {
        return Err(EINVAL);
    }
    let size = mem.data_size(caller.as_context()) as u64;
    let end = (ptr as u64).checked_add(len as u64).ok_or(EFAULT)?;
    if end > size {
        return Err(EFAULT);
    }
    Ok((ptr as usize, len as usize))
}

/// Read `len` bytes from guest memory at `ptr`. Bounds-checked. `len == 0` → empty.
pub fn guest_read(
    caller: &Caller<'_, RuntimeState>,
    ptr: i32,
    len: i32,
) -> Result<Vec<u8>, i32> {
    let mem = memory(caller)?;
    let (off, n) = checked_range(caller, &mem, ptr, len)?;
    let mut buf = alloc::vec![0u8; n];
    if n > 0 {
        mem.read(caller.as_context(), off, &mut buf).map_err(|_| EFAULT)?;
    }
    Ok(buf)
}

/// Read exactly `buf.len()` bytes from guest memory at `ptr` into `buf`.
pub fn guest_read_into(
    caller: &Caller<'_, RuntimeState>,
    ptr: i32,
    buf: &mut [u8],
) -> Result<(), i32> {
    let mem = memory(caller)?;
    let len: i32 = buf.len().try_into().map_err(|_| EINVAL)?;
    let (off, _n) = checked_range(caller, &mem, ptr, len)?;
    if !buf.is_empty() {
        mem.read(caller.as_context(), off, buf).map_err(|_| EFAULT)?;
    }
    Ok(())
}

/// Write `bytes` into guest memory at `ptr`. Bounds-checked. Empty → no-op.
pub fn guest_write(
    caller: &mut Caller<'_, RuntimeState>,
    ptr: i32,
    bytes: &[u8],
) -> Result<(), i32> {
    let mem = memory(caller)?;
    let len: i32 = bytes.len().try_into().map_err(|_| EINVAL)?;
    let (off, _n) = checked_range(caller, &mem, ptr, len)?;
    if !bytes.is_empty() {
        mem.write(caller.as_context_mut(), off, bytes).map_err(|_| EFAULT)?;
    }
    Ok(())
}

/// Write a little-endian u32 scalar at `ptr` (common for *_ptr out-params).
pub fn guest_write_u32(
    caller: &mut Caller<'_, RuntimeState>,
    ptr: i32,
    val: u32,
) -> Result<(), i32> {
    guest_write(caller, ptr, &val.to_le_bytes())
}
```

NOTE: confirm `wasm_memory` helper's actual location/signature in
`host/lifecycle.rs`; if it already returns the `Memory`, you may reuse it inside
`memory()` instead of re-deriving. Confirm `caller.get_export` + `wasmi::Extern`
are the real 1.0.9 names (they are — `Caller::get_export(&self, &str) ->
Option<Extern>`). If `AsContext`/`AsContextMut` import paths differ, fix to
compile.

- [ ] **Step 2: Register the module.** In `kernel/src/wasm/host/mod.rs` add
  `pub mod mem;`.

- [ ] **Step 3: Migrate every host fn site.** For EACH of the 43 sites, replace
  the raw call with the accessor, propagating its errno the way that fn already
  returns errnos. Patterns:
  - Read of known length into a stack/heap buf:
    `mem.read(&caller, p as usize, &mut buf).map_err(...)?`
    → `crate::wasm::host::mem::guest_read_into(&caller, p, &mut buf)?`
    (map the `i32` errno to the fn's existing error type if it returns
    `Result<i32, Error>` — e.g. `.map_err(Error::i32_exit)?` only if that's the
    convention; otherwise `match ... { Err(e) => return Ok(e), Ok(()) => {} }`
    to return the errno as the WASI return value — MATCH each fn's style).
  - Read of guest-controlled length:
    `let buf = match guest_read(&caller, ptr, len) { Ok(b) => b, Err(e) => return Ok(e) };`
  - Write back:
    `if let Err(e) = guest_write(&mut caller, ptr, &bytes) { return Ok(e); }`
  - Scalar out-param: `guest_write_u32(&mut caller, ptr, val)`.
  Do the files in this order (low-risk first): clock.rs (2), random.rs (1),
  term.rs (2), path.rs (2), service.rs (5), sysinfo.rs (7), lifecycle.rs (6),
  proc.rs (11), fd.rs (11). Do NOT regress the just-merged `fd_readdir` /
  `path_open` paths — route their existing reads/writes through the accessor,
  preserving their errnos.

- [ ] **Step 4: Audit grep must be clean.**

Run: `wsl -d Ubuntu -u root -e bash -c "cd /mnt/e/MinimalOS/BasicOperatingSystem && grep -rn 'mem.read\|mem.write\|\.read(&caller\|\.write(&mut caller' kernel/src/wasm/host/ | grep -v 'host/mem.rs'"`
Expected: **no output** (every raw access now lives only in mem.rs).

- [ ] **Step 5: Build.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error\[|error:|Limine BIOS stages installed"'`
Expected: `Limine BIOS stages installed successfully.`, no `error`.

- [ ] **Step 6: Smoke the existing behavior.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -3'`
Expected: `TEST_PASS` (init.sh tools still work → the migration didn't break
real guest I/O).

- [ ] **Step 7: CHANGELOG + commit.**

Create `CHANGELOG/176-26-05-31-host-mem-accessor.md` (format: `# 176 — title`,
`**Data:** 2026-05-31`, `## Cosa`, `## Perché`, `## File toccati` — match
`CHANGELOG/157-*`). Then:
```bash
git add kernel/src/wasm/host/mem.rs kernel/src/wasm/host/ CHANGELOG/176-26-05-31-host-mem-accessor.md
git commit -m "feat(wasm): single audited guest-memory accessor; migrate all host fns"
```

---

## Task 2: Fuel metering + kill-on-exhaustion

**Files:**
- Modify: `kernel/src/wasm/fiber.rs`

**FIRST — read:** `kernel/src/wasm/fiber.rs` lines ~28–60 (Engine/Config/Store
setup in `Fiber::new`) and the run loop (~110–185) including the existing
stubbed out-of-fuel arm and `fn error_to_exit`. Confirm how the resumable call
surfaces errors (`ResumableCall::HostTrap` vs a returned `Err`) so you hook the
out-of-fuel detection at the right place.

- [ ] **Step 1: Enable fuel in Config.** In `Fiber::new`, before `Engine::new`,
  on the `Config` builder add:
```rust
        config.consume_fuel(true);
```
(next to the existing `config.compilation_mode(...)`).

- [ ] **Step 2: Seed the budget after Store creation.** Define near the top of
  fiber.rs:
```rust
/// Per-host-call fuel budget. Pure-compute loops with no host calls burn this
/// and get killed; I/O-bound modules refuel every host call and run forever.
const FUEL_PER_SLICE: u64 = 2_000_000_000;
```
  After the `Store::new(...)` line:
```rust
        let _ = store.set_fuel(FUEL_PER_SLICE);
```
(`set_fuel` returns `Result`; fuel is enabled so it's Ok — ignore.)

- [ ] **Step 3: Refuel at each host-call boundary.** In the run loop, every time
  a `SuspendReason` is dispatched and the fiber is about to `resume`, top the
  budget back up. Find the point right after `self.dispatch(reason).await`
  returns the errno and before `inv = state.resume(...)`; add:
```rust
            let _ = self.store.set_fuel(FUEL_PER_SLICE);
```
  (Re-seeding to a fixed budget each host call is simpler and safer than adding
  deltas; a module doing real work always calls host fns.)

- [ ] **Step 4: Wire the out-of-fuel arm to kill the task.** Where the run loop
  matches the call result, detect the out-of-fuel trap and return a non-zero
  exit instead of looping/hanging. The error is an `Error` with trap code
  `OutOfFuel`. Use:
```rust
                    if err.as_trap_code() == Some(wasmi::core::TrapCode::OutOfFuel) {
                        crate::kprintln!("wasm: task killed (fuel exhausted)");
                        return 137; // 128 + SIGKILL-ish
                    }
```
  Place this in the existing error-handling arm (replace/augment the stubbed
  `:156` fuel arm). Confirm `err.as_trap_code()` exists in 1.0.9
  (`Error::as_trap_code(&self) -> Option<TrapCode>`); if the method name differs
  read `~/.cargo/.../wasmi-1.0.9/src/error.rs` — there's an `is_out_of_fuel`
  but it's `pub(crate)`; `as_trap_code` is the public path. Match the real API.

- [ ] **Step 5: Build.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error\[|error:|Limine BIOS stages installed"'`
Expected: clean.

- [ ] **Step 6: Behavior smoke (manual reasoning + run-test).**
Run-test must still `TEST_PASS` (all init.sh tools are I/O-bound → they refuel).
A dedicated tight-loop `.wasm` test is added in Task 6's integration set; here
just confirm no regression.

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -3'`
Expected: `TEST_PASS`.

- [ ] **Step 7: CHANGELOG + commit.**
Create `CHANGELOG/177-26-05-31-wasm-fuel.md`. Then:
```bash
git add kernel/src/wasm/fiber.rs CHANGELOG/177-26-05-31-wasm-fuel.md
git commit -m "feat(wasm): fuel metering — refuel on host call, kill on exhaustion"
```

---

## Task 3: Per-task resource limits (fd cap, ResourceLimiter, socket cap)

**Files:**
- Modify: `kernel/src/wasm/state.rs`
- Modify: `kernel/src/wasm/fiber.rs`

**FIRST — read:** `~/.cargo/.../wasmi-1.0.9/src/limits.rs` for the exact
`ResourceLimiter` method signatures and return types, and `store/mod.rs:97` for
`Store::limiter`'s closure signature. Also find the two unbounded `fds.push`
sites: `grep -n 'fds.push\|\.push(Some(FdEntry' kernel/src/wasm/fiber.rs`
(spec cites :287 and :314) and any socket-pushing site (`FdEntry::Socket`).

- [ ] **Step 1: Add limit constants + memory counter to state.rs.**
```rust
/// Max simultaneous FDs per task. Past this, fd-allocating host fns return EMFILE.
pub const MAX_FDS: usize = 128;
/// Max simultaneous kernel sockets per task.
pub const MAX_SOCKETS: usize = 16;
/// Per-task linear-memory ceiling in bytes (wasmi ResourceLimiter).
pub const MAX_LINEAR_MEM: usize = 64 * 1024 * 1024;
```

- [ ] **Step 2: Implement ResourceLimiter on RuntimeState.** Add to state.rs
  (adjust method signatures to the 1.0.9 trait you just read — the bodies are
  the intent):
```rust
impl wasmi::ResourceLimiter for RuntimeState {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> Result<bool, wasmi::errors::MemoryError> {
        let cap = maximum.map(|m| m.min(MAX_LINEAR_MEM)).unwrap_or(MAX_LINEAR_MEM);
        Ok(desired <= cap)
    }
    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> Result<bool, wasmi::errors::TableError> {
        let cap = maximum.unwrap_or(4096);
        Ok(desired <= cap)
    }
}
```
  (If 1.0.9's trait returns `Result<bool, wasmi::Error>` or a different error
  type, or names the params differently, match it exactly. The `Ok(false)`
  return denies the growth → wasm sees the allocation fail, task continues or
  traps, kernel survives.)

- [ ] **Step 3: Attach the limiter in `Fiber::new`.** After `Store::new` (and
  `set_fuel` from Task 2):
```rust
        store.limiter(|state| state as &mut dyn wasmi::ResourceLimiter);
```
  (Confirm the closure return shape against store/mod.rs:97.)

- [ ] **Step 4: Bound the fd table.** Add a helper in fiber.rs (or inline at the
  two sites). Replace each `state.fds.push(Some(FdEntry::...))` /
  `caller.data_mut().fds.push(...)` allocation with: first scan for a `None`
  slot and reuse it; else push ONLY if `fds.len() < MAX_FDS`; else return EMFILE
  (24). Concretely, where a new fd is allocated:
```rust
        // find free slot or extend (bounded)
        let slot = {
            let fds = &mut /* state or caller.data_mut() */ .fds;
            match fds.iter().position(|s| s.is_none()) {
                Some(i) => { fds[i] = Some(entry); i }
                None if fds.len() < crate::wasm::state::MAX_FDS => {
                    fds.push(Some(entry)); fds.len() - 1
                }
                None => return /* EMFILE in this fn's convention */ ,
            }
        };
```
  Apply at BOTH spec sites (:287, :314) and any other fd-creating host fn that
  pushes (check fd.rs path_open/OpenDir, proc.rs tcp_dial-style). Reuse a single
  helper if the surrounding types allow (DRY).

- [ ] **Step 5: Cap sockets.** Where `FdEntry::Socket` is created, before
  allocating count existing `FdEntry::Socket` in `fds`; if `>= MAX_SOCKETS`
  return EMFILE/ENFILE per that fn's convention.

- [ ] **Step 6: Build + smoke.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error\[|error:|Limine BIOS stages installed" && make run-test 2>&1 | tail -3'`
Expected: clean build + `TEST_PASS`.

- [ ] **Step 7: CHANGELOG + commit.**
Create `CHANGELOG/178-26-05-31-wasm-resource-limits.md`. Then:
```bash
git add kernel/src/wasm/state.rs kernel/src/wasm/fiber.rs CHANGELOG/178-26-05-31-wasm-resource-limits.md
git commit -m "feat(wasm): per-task limits — fd cap, linear-mem limiter, socket cap"
```

---

## Task 4: Capability-scoped preopens (enforce against "/" grant first)

**Files:**
- Modify: `kernel/src/wasm/state.rs` (add grant field)
- Modify: `kernel/src/wasm/host/path.rs` and the OpenDir/path_open paths in
  `kernel/src/wasm/host/fd.rs`

**FIRST — read:** `kernel/src/wasm/host/path.rs` and the `path_open`/`OpenDir`
handlers (in fd.rs and/or the fiber dispatch arms — `grep -rn 'resolve_cwd\|PathOpen\|OpenDir\|path_open' kernel/src/wasm/`).
Find the existing `resolve_cwd` (it's in `host/proc.rs` per the SSH work) — it
already collapses `.`/`..` to an absolute path. Reuse it; do NOT write a second
canonicalizer.

- [ ] **Step 1: Add the grant to RuntimeState.** In state.rs struct + `new()`:
```rust
    /// Capability grant: absolute path prefix this task may access. Default "/"
    /// (full FS, no behavior change). Narrowed for spawned tools later.
    pub root: String,
```
  In `RuntimeState::new()` set `root: String::from("/")` in the struct literal.

- [ ] **Step 2: Add a grant check helper** (in state.rs or a small
  `host/cap.rs` — keep it next to where it's used; state.rs is fine):
```rust
impl RuntimeState {
    /// True if `abs` (an already-canonicalized absolute path) is within this
    /// task's grant. "/" grants everything. Prevents `../` escapes because the
    /// caller canonicalizes first.
    pub fn grants(&self, abs: &str) -> bool {
        if self.root == "/" { return true; }
        abs == self.root
            || abs.starts_with(&alloc::format!("{}/", self.root.trim_end_matches('/')))
    }
}
```

- [ ] **Step 3: Enforce in path handlers.** In `path_open` and `OpenDir` (and any
  other `path_*` that resolves a guest path), AFTER computing the canonical
  absolute path via the existing `resolve_cwd(&caller.data().cwd, path)`:
```rust
        let abs = resolve_cwd(&caller.data().cwd, path);
        if !caller.data().grants(&abs) {
            return Ok(76); // ENOTCAPABLE
        }
```
  Use the path each handler already computes — do not re-resolve differently.
  With the default `root="/"`, `grants` always returns true → **zero behavior
  change**, but the enforcement code is now in place and testable.

- [ ] **Step 4: Build + smoke.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error\[|error:|Limine BIOS stages installed" && make run-test 2>&1 | tail -3'`
Expected: clean + `TEST_PASS` (default "/" grant → no tool breaks).

- [ ] **Step 5: CHANGELOG + commit.**
Create `CHANGELOG/179-26-05-31-wasm-capability-scoping.md`. Then:
```bash
git add kernel/src/wasm/state.rs kernel/src/wasm/host/path.rs kernel/src/wasm/host/fd.rs CHANGELOG/179-26-05-31-wasm-capability-scoping.md
git commit -m "feat(wasm): capability-scoped path grants (enforced against / grant)"
```

NOTE (follow-up, NOT this task): narrowing the grant for spawned tools needs the
spawn API to pass a `root`; defer until `proc_spawn` exists. Document this in the
CHANGELOG as a known follow-up.

---

## Task 5: Survivable panic path

**Files:**
- Modify: `kernel/src/boot/panic.rs`

**FIRST — read:** `kernel/src/boot/panic.rs` (the current handler). Confirm:
the serial lock API (`crate::serial::SERIAL` — does it expose `try_lock()`?),
the klog/dmesg ring buffer API added recently (`grep -rn 'klog\|dmesg\|KLOG' kernel/src/`),
and the console lock. Confirm a reset primitive exists or add one (0xCF9 port
write is simplest: `outb(0xCF9, 0x0E)`).

- [ ] **Step 1: Rewrite the handler.** Replace the body with a non-deadlocking,
  observable, resetting path:
```rust
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    use core::fmt::Write;
    x86_64::instructions::interrupts::disable();

    // Append to the klog ring (best-effort) so dmesg/serial can show it.
    // (Use the real klog API confirmed above.)
    // crate::klog::record_fmt(format_args!("KERNEL PANIC: {}", info));

    // Serial: try_lock only — never block (we may be holding it already).
    if let Some(mut s) = crate::serial::SERIAL.try_lock() {
        let _ = writeln!(s, "\nKERNEL PANIC: {}", info);
    }

    // Console: try_lock only.
    if let Some(mut c) = crate::console::CONSOLE.try_lock() {
        let _ = writeln!(c, "KERNEL PANIC: {}", info);
    }

    #[cfg(feature = "panic-halt")]
    loop { x86_64::instructions::hlt(); }

    #[cfg(not(feature = "panic-halt"))]
    {
        // Controlled reset via the 0xCF9 reset control register.
        unsafe {
            use x86_64::instructions::port::Port;
            let mut p: Port<u8> = Port::new(0xCF9);
            p.write(0x0E); // request hard reset
        }
        // If reset didn't take, halt rather than spin hot.
        loop { x86_64::instructions::hlt(); }
    }
}
```
  Adjust to the REAL lock APIs: if `spin::Mutex` is used, `try_lock()` returns
  `Option<Guard>` — good. If `SERIAL`/`CONSOLE` are wrapped differently, match.
  If a klog API exists, wire it; if not, skip that line (don't invent one).

- [ ] **Step 2: Add the `panic-halt` feature.** In `kernel/Cargo.toml`
  `[features]`, add `panic-halt = []`. Default build resets; dev can
  `--features panic-halt` to halt-for-inspection. (Check whether the existing
  test harness needs halt — if `make run-test` relies on the box halting on a
  test-success panic, gate accordingly or keep success path non-panic.)

- [ ] **Step 3: Build (default + feature).**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error\[|error:|Limine BIOS stages installed"'`
Expected: clean.

- [ ] **Step 4: Smoke that normal boot/tests still pass.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -3'`
Expected: `TEST_PASS` (panic path not exercised by the happy path).

- [ ] **Step 5: CHANGELOG + commit.**
Create `CHANGELOG/180-26-05-31-survivable-panic.md`. Then:
```bash
git add kernel/src/boot/panic.rs kernel/Cargo.toml CHANGELOG/180-26-05-31-survivable-panic.md
git commit -m "feat(boot): non-deadlocking panic — try_lock serial/console + reset"
```

---

## Task 6: Host-boundary fuzz harness + integration kill/limit tests

**Files:**
- Create: `#[cfg(test)]` module in `kernel/src/wasm/host/mem.rs` (or
  `kernel/src/wasm/host/mem_tests.rs` included via `#[cfg(test)] mod`)
- Create: integration `.wasm` test programs under `user/` + a `tests/*.sh`
- Modify: `Makefile` (a `test-host` target if not present)

**FIRST — check:** can the kernel crate compile a `#[cfg(test)]` host-target
test at all? It's `no_std` + a custom target. If `cargo test` on the kernel
target is not feasible, put the fuzz tests in a SMALL separate host crate
(`std`) that depends on `wasmi` only and re-declares a minimal `RuntimeState`
stub + the `mem.rs` logic — OR factor the pure bound-check (`checked_range`
operating on a `len: i32, ptr: i32, size: u64`) into a tiny pure function with
no wasmi types and unit-test THAT exhaustively. **Prefer the pure-function
split** — it's the real logic and is trivially host-testable. Read the existing
test setup (`grep -rn '#\[cfg(test)\]\|#\[test\]' kernel/src/ user/`) to see
what's already done.

- [ ] **Step 1: Factor the pure bound check.** In mem.rs, extract:
```rust
/// Pure bound check — no wasmi types, host-testable. Returns Ok((off,len)) or errno.
pub(crate) fn check_bounds(ptr: i32, len: i32, size: u64) -> Result<(usize, usize), i32> {
    if ptr < 0 || len < 0 { return Err(EINVAL); }
    let end = (ptr as u64).checked_add(len as u64).ok_or(EFAULT)?;
    if end > size { return Err(EFAULT); }
    Ok((ptr as usize, len as usize))
}
```
  and call it from `checked_range` (which only adds the live `data_size()`).

- [ ] **Step 2: Adversarial unit tests.** Add to mem.rs:
```rust
#[cfg(test)]
mod tests {
    use super::{check_bounds, EINVAL, EFAULT};

    #[test]
    fn negative_ptr_or_len_rejected() {
        assert_eq!(check_bounds(-1, 0, 100), Err(EINVAL));
        assert_eq!(check_bounds(0, -1, 100), Err(EINVAL));
        assert_eq!(check_bounds(i32::MIN, 0, 100), Err(EINVAL));
    }
    #[test]
    fn overflow_rejected() {
        assert_eq!(check_bounds(i32::MAX, i32::MAX, u64::MAX), Ok((i32::MAX as usize, i32::MAX as usize)));
        // ptr+len within u64 never overflows for i32 inputs, but ranges past size are EFAULT:
        assert_eq!(check_bounds(i32::MAX, i32::MAX, 10), Err(EFAULT));
    }
    #[test]
    fn straddling_end_rejected() {
        assert_eq!(check_bounds(90, 20, 100), Err(EFAULT));
        assert_eq!(check_bounds(100, 1, 100), Err(EFAULT));
    }
    #[test]
    fn zero_len_ok_even_at_boundary() {
        assert_eq!(check_bounds(100, 0, 100), Ok((100, 0)));
        assert_eq!(check_bounds(0, 0, 0), Ok((0, 0)));
    }
    #[test]
    fn in_range_ok() {
        assert_eq!(check_bounds(0, 100, 100), Ok((0, 100)));
        assert_eq!(check_bounds(50, 50, 100), Ok((50, 100 - 50)));
    }
}
```
  These must compile on the host. If the kernel crate can't `cargo test` due to
  the custom target, move `check_bounds` + tests into a tiny `#[cfg(test)]`-only
  path that builds under the host target, or a sibling `kernel/tests/` host
  crate. Pick the approach that actually runs and document it.

- [ ] **Step 3: Run the host tests.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem/kernel && source $HOME/.cargo/env && cargo test --lib check_bounds 2>&1 | tail -20'`
Expected: tests pass. (If the kernel lib can't host-test, run whatever harness
Step 2 settled on and paste the green result.)

- [ ] **Step 4: Integration `.wasm` — fuel kill.** Create `user/spinloop/` (a
  WASI bin whose `main` does `loop {}`) + add to the user build. Create
  `tests/fuel-test.sh` (mirror `tests/ssh-shell-test.sh` harness): boot, run
  `/bin/spinloop.wasm` (locally via init.sh or over SSH), assert serial contains
  `wasm: task killed (fuel exhausted)` AND a subsequent shell prompt appears
  (kernel still alive). Add `run-fuel-test` to the Makefile.

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-fuel-test 2>&1 | tail -10'`
Expected: marker present + kernel responsive → print `TEST_PASS_FUEL`.

- [ ] **Step 5: All existing tests still pass.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -3 && make run-ssh-test 2>&1 | tail -3'`
Expected: `TEST_PASS` and `TEST_PASS_SSH`.

- [ ] **Step 6: README honest-ceiling section + CHANGELOG + commit.**
Add a `## Security model` section to README.md stating: defense-in-depth in
ring 0; host-boundary is one audited accessor + fuzzed; fuel/limits/scoping/
panic-reset bound the blast radius; **but a memory-safety bug inside wasmi
itself or the kernel's own `unsafe` is still fatal — only a separate address
space + CPU privilege level (explicitly out of scope) would contain that.**
Create `CHANGELOG/181-26-05-31-host-boundary-fuzz.md`. Then:
```bash
git add kernel/src/wasm/host/mem.rs user/spinloop tests/fuel-test.sh Makefile README.md CHANGELOG/181-26-05-31-host-boundary-fuzz.md
git commit -m "test(wasm): host-boundary bound-check fuzz + fuel-kill integration; README ceiling"
```

---

## Self-review notes (addressed)

- **Spec coverage:** mem accessor + migrate (T1) ✓; fuel + kill (T2) ✓; fd cap +
  ResourceLimiter + socket cap (T3) ✓; capability scoping against "/" (T4) ✓;
  survivable panic (T5) ✓; fuzz harness + README ceiling + existing tests green
  (T6) ✓. Grep-clean done-criterion = T1 Step 4. ENOTCAPABLE/EMFILE/EFAULT
  errnos consistent (28/21/24/76/2) across tasks.
- **API risk flagged, not hand-waved:** every wasmi 1.0.9 call the plan emits is
  backed by a confirmed signature OR carries an explicit "read this crate file
  and match" instruction at the exact point of use (ResourceLimiter return type,
  `as_trap_code`, `Store::limiter` closure shape).
- **Ordering:** T1 first (accessor) so T2–T5 host-side additions use it.
- **No-regression guard:** every task re-runs `run-test`; T6 also `run-ssh-test`.
- **Known follow-up:** narrowing grants for spawned tools (needs proc_spawn) —
  flagged in T4, not silently dropped.
