# SMP Fase 2 — Kernel compute offload pool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the idle Application Processors into a worker pool that executes pure-CPU kernel jobs from an SMP-safe queue in parallel with the BSP, with a `smptest` tool that measures real parallel speedup.

**Architecture:** A fixed array of job slots + an `IrqMutex<VecDeque<usize>>` queue of slot ids. The BSP `submit`s jobs (a `fn(&[u8]) -> u64` + `'static` input); each AP, instead of `hlt`, runs a worker loop that `take`s a slot, runs it synchronously on its core, and `complete`s it (result + which cpu ran it). The BSP polls for completion. No `.wasm`, no STI/IPI/preemption on APs — pure spin-wait worker pool. BSP cooperative executor untouched.

**Tech Stack:** Rust `no_std`, `crate::sync::IrqMutex` (Fase 0), atomics, `crate::cpu::{cpu_id, cpus_online}`, `crate::boot::clock::elapsed_ms` (TSC timing), wasmi host fn + a `smptest` WASI tool.

---

## Confirmed facts (verified against the tree — use these)

- **TSC timing source:** `crate::boot::clock::elapsed_ms() -> u64` (boot/clock.rs:75) is public, TSC-based, lock-free, IRQ-free — safe to call from the BSP for benchmarking. (The 100 Hz `timer::ticks()` = 10 ms granularity is too coarse; use `elapsed_ms()`.)
- **`ap_entry`** (cpu/ap.rs) currently ends in `loop { x86_64::instructions::hlt(); }`. Fase 2 replaces that tail with `ap_worker_loop()`.
- **`IrqMutex<T>`** (kernel/src/sync/mod.rs): `pub const fn new(val)`, `pub fn lock() -> IrqGuard`, `pub fn try_lock() -> Option<IrqGuard>`. IrqGuard derefs to T.
- **`crate::cpu::cpu_id() -> u32`** (LAPIC-based, works on any core), **`cpu::cpus_online() -> u32`** (count of online APs; total CPUs = 1 + this).
- **Host fn pattern:** `kernel/src/wasm/host/sysinfo.rs` has `write_bytes_and_len(caller, buf_ptr, buf_len, used_ptr, &bytes)` and fns like `ruos_cpuinfo` registered in `link()` via `.func_wrap("ruos", "name", fn)?`. A user tool calls it as an `extern "C"` import. Mirror this for `ruos_smp_bench`.
- **User tool pattern:** `user/lscpu/` is a minimal WASI bin calling a `ruos` host fn. Mirror for `user/smptest/`. The Makefile builds `user/*` into `user-bin/*.wasm` and mounts to `/bin/` (check `BIN_TOOLS` in the Makefile + how lscpu is listed).

Build env: **PowerShell tool** for WSL (`make iso` → `Limine BIOS stages installed successfully.`). After committing, `touch kernel/build.rs` before `make iso` so the banner sha matches HEAD. **Bash tool** for git. CHANGELOG counter: highest is 193 → use 194+.

VBox harness: VBoxManage `C:\Program Files\Oracle\VirtualBox\VBoxManage.exe`; VM `ruos`, serial→`build/log-vbox.log`, 6 vCPU. `& $vbm startvm ruos --type headless; Start-Sleep 16; & $vbm controlvm ruos poweroff` then read log.

---

## File structure

- Create `kernel/src/smp/pool.rs` — JobSlot array, IrqMutex queue, submit/take/run_slot/complete/poll_done.
- Modify `kernel/src/smp.rs` — `pub mod pool;` (smp.rs currently is a single-file module; convert to `smp/mod.rs` OR add a sibling — see Task 1 Step 1).
- Modify `kernel/src/cpu/ap.rs` — `ap_worker_loop` replaces the idle hlt tail.
- Modify `kernel/src/wasm/host/sysinfo.rs` (or a new `host/smp.rs`) — `ruos_smp_bench` host fn + the hash job + register in `link`.
- Create `user/smptest/` — the tool. Modify `user/Cargo.toml` + `Makefile`.
- Create `tests/smp2-test.sh` + Makefile `run-smp2-test`.
- `CHANGELOG/NN-26-06-01-*.md` per task; roadmap note.

---

## Task 1: Job pool — slots + queue + submit/take/complete

**Files:**
- Create: `kernel/src/smp/pool.rs`
- Modify: `kernel/src/smp.rs` → convert to module dir OR declare submodule

**FIRST:** `kernel/src/smp.rs` is a single file. To add `pool` as a submodule, the cleanest is: move `kernel/src/smp.rs` → `kernel/src/smp/mod.rs`, then add `pub mod pool;` at its top. Do that move (git mv) so `crate::smp::bringup` stays valid and `crate::smp::pool::*` works. Confirm `mod smp;` in main.rs still resolves to the dir module (it does — Rust resolves `mod smp;` to either `smp.rs` or `smp/mod.rs`).

- [ ] **Step 1: Convert smp.rs to a module dir + add pool.**
```bash
git mv kernel/src/smp.rs kernel/src/smp/mod.rs
```
Then at the top of `kernel/src/smp/mod.rs` add:
```rust
pub mod pool;
```

- [ ] **Step 2: Create `kernel/src/smp/pool.rs`.**
```rust
//! Kernel compute offload pool. A fixed array of job slots + a queue of slot
//! ids (IrqMutex). The BSP `submit`s pure-CPU jobs; AP worker loops `take` and
//! run them on their core, then `complete`. No I/O, no shared mutable state in
//! a job — pure functions over `'static` immutable input.

use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use alloc::collections::VecDeque;
use crate::sync::IrqMutex;

/// Max in-flight jobs.
pub const MAX_JOBS: usize = 64;

// Slot states.
const EMPTY: u8 = 0;
const QUEUED: u8 = 1;
const RUNNING: u8 = 2;
const DONE: u8 = 3;

/// A pure-CPU job: `fn(&[u8]) -> u64`. No captures, no I/O, no blocking.
pub type JobFn = fn(&[u8]) -> u64;

struct JobSlot {
    state: AtomicU8,
    work: AtomicUsize,    // JobFn as usize (fn pointer)
    input_ptr: AtomicUsize,
    input_len: AtomicUsize,
    result: AtomicU64,
    ran_on: AtomicU32,    // cpu_id that executed it
}

impl JobSlot {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(EMPTY),
            work: AtomicUsize::new(0),
            input_ptr: AtomicUsize::new(0),
            input_len: AtomicUsize::new(0),
            result: AtomicU64::new(0),
            ran_on: AtomicU32::new(u32::MAX),
        }
    }
}

static SLOTS: [JobSlot; MAX_JOBS] = {
    const S: JobSlot = JobSlot::new();
    [S; MAX_JOBS]
};

/// Queue of QUEUED slot ids, in submission order.
static QUEUE: IrqMutex<VecDeque<usize>> = IrqMutex::new(VecDeque::new());

/// Submit a pure-CPU job. `input` must be `'static` (lives until the job is
/// done). Returns the slot id, or None if the pool is full.
pub fn submit(work: JobFn, input: &'static [u8]) -> Option<usize> {
    // Claim a free slot via CAS EMPTY->QUEUED.
    for (id, slot) in SLOTS.iter().enumerate() {
        if slot.state.compare_exchange(EMPTY, QUEUED, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            slot.work.store(work as usize, Ordering::SeqCst);
            slot.input_ptr.store(input.as_ptr() as usize, Ordering::SeqCst);
            slot.input_len.store(input.len(), Ordering::SeqCst);
            slot.ran_on.store(u32::MAX, Ordering::SeqCst);
            QUEUE.lock().push_back(id);
            return Some(id);
        }
    }
    None
}

/// Take a QUEUED slot id off the queue (called by AP workers and the BSP
/// fallback). CAS QUEUED->RUNNING; returns the id if claimed.
pub fn take() -> Option<usize> {
    let id = QUEUE.lock().pop_front()?;
    if SLOTS[id].state.compare_exchange(QUEUED, RUNNING, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        Some(id)
    } else {
        None // someone else grabbed it (shouldn't happen with single dequeue)
    }
}

/// Run the slot's job on the current core and mark it DONE.
pub fn run_slot(id: usize, cpu: u32) {
    let slot = &SLOTS[id];
    let work_addr = slot.work.load(Ordering::SeqCst);
    let ptr = slot.input_ptr.load(Ordering::SeqCst) as *const u8;
    let len = slot.input_len.load(Ordering::SeqCst);
    // SAFETY: input was a `'static [u8]` passed to submit; ptr/len reconstruct
    // the same slice, valid until the BSP frees the slot after DONE.
    let input: &[u8] = unsafe { core::slice::from_raw_parts(ptr, len) };
    // SAFETY: work_addr is a `JobFn` (fn(&[u8])->u64) stored by submit.
    let work: JobFn = unsafe { core::mem::transmute::<usize, JobFn>(work_addr) };
    let result = work(input);
    slot.result.store(result, Ordering::SeqCst);
    slot.ran_on.store(cpu, Ordering::SeqCst);
    slot.state.store(DONE, Ordering::SeqCst);
}

/// If slot `id` is DONE, return (result, ran_on cpu) and free the slot.
/// Returns None if not done yet.
pub fn poll_done(id: usize) -> Option<(u64, u32)> {
    let slot = &SLOTS[id];
    if slot.state.load(Ordering::SeqCst) == DONE {
        let r = slot.result.load(Ordering::SeqCst);
        let c = slot.ran_on.load(Ordering::SeqCst);
        slot.state.store(EMPTY, Ordering::SeqCst); // free
        Some((r, c))
    } else {
        None
    }
}
```
NOTE on the `fn`-as-`usize` round-trip: a non-capturing `fn(&[u8]) -> u64` is a
plain code pointer; `work as usize` then `transmute::<usize, JobFn>` is sound on
this target (function pointers are thin). The `transmute` is the documented
unsafe boundary. If the compiler rejects `transmute::<usize, JobFn>`, use
`core::mem::transmute::<usize, JobFn>(work_addr)` with an intermediate
`*const ()` cast, or store the fn pointer in an `AtomicPtr<()>` instead of
`AtomicUsize` and cast. Make it compile soundly.

- [ ] **Step 3: Build.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error\[|error:|Limine BIOS stages installed"'`
Expected: clean. (submit/take/run_slot/poll_done unused until Task 2/3 — warnings fine.)

- [ ] **Step 4: Commit.**
Create `CHANGELOG/194-26-06-01-smp-pool.md` (format like CHANGELOG/193). Summarize: smp/pool.rs — job slot array + IrqMutex queue; submit/take/run_slot/complete/poll_done; pure-CPU jobs (fn(&[u8])->u64) over 'static input; smp.rs → smp/mod.rs. Then:
```bash
git add kernel/src/smp/ CHANGELOG/194-26-06-01-smp-pool.md
git commit -m "feat(smp): kernel compute job pool — slots + IrqMutex queue"
```

---

## Task 2: AP worker loop

**Files:**
- Modify: `kernel/src/cpu/ap.rs`

- [ ] **Step 1: Replace the idle hlt tail with a worker loop.** In `kernel/src/cpu/ap.rs`, change `ap_entry`'s final `loop { hlt }` to call `ap_worker_loop()`, and add the loop fn:
```rust
pub unsafe extern "C" fn ap_entry(info: &MpInfo) -> ! {
    let cpu_id = info.extra_argument() as usize;
    crate::gdt::init(cpu_id);
    crate::idt::load();
    crate::cpu::mark_online();
    ap_worker_loop()
}

/// AP worker loop: take pure-CPU jobs from the pool and run them on this core.
/// Spin-waits (PAUSE) when there's no work — no STI/IPI in Fase 2, so the AP
/// polls the queue rather than sleeping on an interrupt.
fn ap_worker_loop() -> ! {
    let me = crate::cpu::cpu_id();
    loop {
        match crate::smp::pool::take() {
            Some(slot) => crate::smp::pool::run_slot(slot, me),
            None => core::hint::spin_loop(),
        }
    }
}
```
Keep the doc comment header accurate: update it to say the AP runs a compute
worker loop (Fase 2) rather than parking idle (Fase 1).

- [ ] **Step 2: Build.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error\[|error:|Limine BIOS stages installed"'`
Expected: clean.

- [ ] **Step 3: Smoke — APs still come online + boot OK.**
Run: `wsl -d Ubuntu -u root -e bash -c 'pkill -9 qemu-system-x86; sleep 2; cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 25 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -boot d -cdrom build/os.iso -serial stdio -display none -no-reboot -m 256 -drive file=build/disk.img,format=raw,if=none,id=disk0 -device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0 2>&1 | grep -iE "APs online|#PF|panic|init.sh complete"'`
Expected: `smp: 3/3 APs online`, `init.sh complete`, NO #PF/panic. (APs now spin in the worker loop instead of hlt — still come online, boot unaffected.)

- [ ] **Step 4: Commit.**
Create `CHANGELOG/195-26-06-01-ap-worker-loop.md`. Summarize: ap_entry now runs ap_worker_loop (take+run jobs from the pool, PAUSE-spin when idle) instead of pure hlt; no STI/IPI. Then:
```bash
git add kernel/src/cpu/ap.rs CHANGELOG/195-26-06-01-ap-worker-loop.md
git commit -m "feat(cpu): AP worker loop — run pool jobs (replaces idle hlt)"
```

---

## Task 3: Benchmark host fn (parallel vs sequential)

**Files:**
- Create: `kernel/src/wasm/host/smp.rs`
- Modify: `kernel/src/wasm/host/mod.rs` (add `pub mod smp;`) and the `link()` that registers host fns (find it: `grep -rn 'func_wrap.*cpuinfo\|fn link' kernel/src/wasm/host/`)

- [ ] **Step 1: Create `kernel/src/wasm/host/smp.rs`.**
```rust
//! Host fn `ruos_smp_bench`: run N identical pure-CPU hash jobs via the SMP
//! pool (parallel across APs) and inline on the BSP (sequential), time both,
//! and report the speedup + the set of cpu_ids that ran the parallel jobs.

use wasmi::{Caller, Error};
use alloc::string::String;
use core::fmt::Write as _;
use crate::wasm::state::RuntimeState;

/// Iterations per job — tuned so one job is ~tens of ms (measurable, not too
/// long for tests). Adjust if jobs are too fast/slow to time.
const ITERS: u64 = 4_000_000;

/// Fixed input buffer the jobs hash over. `'static` so it can be handed to the
/// pool. Content is arbitrary but constant (deterministic result).
static JOB_INPUT: [u8; 64] = [0x5a; 64];

/// Pure-CPU job: a heavy integer-mixing hash over `input`. No I/O, no shared
/// state — safe to run on any core.
fn hash_job(input: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut i: u64 = 0;
    while i < ITERS {
        let mut k = 0usize;
        while k < input.len() {
            h = (h ^ input[k] as u64).wrapping_mul(0x100000001b3);
            k += 1;
        }
        h = h.rotate_left(13).wrapping_add(0x9e3779b97f4a7c15);
        i += 1;
    }
    h
}

/// ruos_smp_bench(buf_ptr, buf_len, used_ptr) -> errno.
/// Writes a one-line ASCII report to the guest buffer:
///   "parallel=Xms sequential=Yms speedup=Z.ZZx cores=[a,b,c]"
pub fn ruos_smp_bench(
    caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
    used_ptr: i32,
) -> Result<i32, Error> {
    let n_aps = crate::cpu::cpus_online();
    // Number of jobs = number of worker cores available (at least 1), capped to
    // a small batch so the bench is quick.
    let n_jobs: usize = if n_aps == 0 { 4 } else { (n_aps as usize).min(8) * 2 };

    // --- Parallel: submit all jobs, then drain results. ---
    let t0 = crate::boot::clock::elapsed_ms();
    let mut ids: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
    for _ in 0..n_jobs {
        match crate::smp::pool::submit(hash_job, &JOB_INPUT) {
            Some(id) => ids.push(id),
            None => break, // pool full — fewer jobs, still valid
        }
    }
    // If there are no APs, drain inline on the BSP so we don't deadlock.
    if n_aps == 0 {
        while let Some(slot) = crate::smp::pool::take() {
            crate::smp::pool::run_slot(slot, crate::cpu::cpu_id());
        }
    }
    // Collect results + the cores that ran them.
    let mut cores: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
    for &id in &ids {
        loop {
            if let Some((_r, c)) = crate::smp::pool::poll_done(id) {
                if !cores.contains(&c) { cores.push(c); }
                break;
            }
            core::hint::spin_loop();
        }
    }
    let parallel_ms = crate::boot::clock::elapsed_ms().saturating_sub(t0);

    // --- Sequential: run the same n_jobs inline on the BSP. ---
    let t1 = crate::boot::clock::elapsed_ms();
    let mut acc: u64 = 0;
    for _ in 0..n_jobs {
        acc = acc.wrapping_add(hash_job(&JOB_INPUT));
    }
    let sequential_ms = crate::boot::clock::elapsed_ms().saturating_sub(t1);
    let _ = acc;

    // --- Report. ---
    let speedup_x100 = if parallel_ms == 0 { 0 } else { sequential_ms * 100 / parallel_ms };
    let mut s = String::new();
    let _ = write!(s, "parallel={}ms sequential={}ms speedup={}.{:02}x cores=[",
        parallel_ms, sequential_ms, speedup_x100 / 100, speedup_x100 % 100);
    cores.sort_unstable();
    for (i, c) in cores.iter().enumerate() {
        if i > 0 { let _ = write!(s, ","); }
        let _ = write!(s, "{}", c);
    }
    let _ = write!(s, "]");

    crate::wasm::host::sysinfo::write_bytes_and_len(caller, buf_ptr, buf_len, used_ptr, s.as_bytes())
}
```
CONFIRM `crate::wasm::host::sysinfo::write_bytes_and_len` is `pub` (it's `fn`
at sysinfo.rs:14 — if it's private, either make it `pub(crate)` or inline the
equivalent guest_write + used_ptr write using `crate::wasm::host::mem::guest_write`
+ `guest_write_u32`, matching how other host fns return text). Match the real
signature.

- [ ] **Step 2: Register the module + host fn.** In `kernel/src/wasm/host/mod.rs` add `pub mod smp;`. In the `link()` builder chain (where `ruos_cpuinfo` etc. are registered via `.func_wrap("ruos", "cpuinfo", ...)?`), add:
```rust
        .func_wrap("ruos", "smp_bench", crate::wasm::host::smp::ruos_smp_bench)?
```
Find the exact `link` location: `grep -rn 'func_wrap("ruos", "cpuinfo"' kernel/src/wasm/host/`.

- [ ] **Step 3: Build.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error\[|error:|Limine BIOS stages installed"'`
Expected: clean.

- [ ] **Step 4: Commit.**
Create `CHANGELOG/196-26-06-01-smp-bench-hostfn.md`. Summarize: ruos_smp_bench host fn — runs N hash jobs via the pool (parallel) vs inline (sequential), times both via TSC clock, reports speedup + distinct cores. Then:
```bash
git add kernel/src/wasm/host/smp.rs kernel/src/wasm/host/mod.rs kernel/src/wasm/host/sysinfo.rs CHANGELOG/196-26-06-01-smp-bench-hostfn.md
git commit -m "feat(wasm): ruos_smp_bench host fn — parallel vs sequential timing"
```

---

## Task 4: smptest user tool

**Files:**
- Create: `user/smptest/Cargo.toml`, `user/smptest/src/main.rs`
- Modify: `user/Cargo.toml` (workspace members), `Makefile` (BIN_TOOLS)

**FIRST — read** `user/lscpu/Cargo.toml` + `user/lscpu/src/main.rs` (the closest sibling — a tool calling a `ruos` text host fn) and how the Makefile lists `lscpu` in `BIN_TOOLS` (grep: `grep -n 'lscpu\|BIN_TOOLS' Makefile`). Mirror exactly.

- [ ] **Step 1: Create `user/smptest/Cargo.toml`** mirroring `user/lscpu/Cargo.toml` (same `[package]`/`[dependencies]`/profile, name `smptest`).

- [ ] **Step 2: Create `user/smptest/src/main.rs`** (mirror lscpu's host-fn call pattern):
```rust
#[link(wasm_import_module = "ruos")]
extern "C" {
    fn smp_bench(buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
}

fn main() {
    let mut buf = vec![0u8; 256];
    let mut used: u32 = 0;
    let errno = unsafe {
        smp_bench(buf.as_mut_ptr() as u32, buf.len() as u32, &mut used as *mut u32 as u32)
    };
    if errno != 0 {
        eprintln!("smptest: errno {}", errno);
        std::process::exit(1);
    }
    let report = std::str::from_utf8(&buf[..used as usize]).unwrap_or("?");
    println!("{}", report);
}
```

- [ ] **Step 3: Wire the build.** Add `"smptest"` to `user/Cargo.toml` workspace members; add `smptest` to the Makefile's `BIN_TOOLS` (the list that gets built to `user-bin/*.wasm` and mounted to `/bin/`). Match how `lscpu` is listed exactly.

- [ ] **Step 4: Build (rebuilds the tool + mounts it).**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error|smptest.wasm|Limine BIOS stages installed"'`
Expected: clean + `smptest.wasm` mounted.

- [ ] **Step 5: Manual smoke — run smptest over SSH on -smp 4.**
Boot QEMU -smp 4 with the SSH hostfwd (mirror tests/ssh-shell-test.sh's QEMU line + the held-open-stdin SSH pattern), run `smptest`, capture output. Expected something like:
`parallel=20ms sequential=58ms speedup=2.90x cores=[1,2,3]` — speedup > 1, ≥2 distinct cores. If speedup ≈ 1 or cores has 1 entry: the jobs aren't running on APs — debug (are APs in the worker loop? is submit reaching the queue? is take() racing?). Report honestly; do NOT fake. (Paste the real smptest output.)

- [ ] **Step 6: Commit.**
Create `CHANGELOG/197-26-06-01-smptest-tool.md`. Summarize: smptest tool prints the smp_bench report (parallel/sequential/speedup/cores). Then:
```bash
git add user/smptest user/Cargo.toml Makefile user-bin/smptest.wasm CHANGELOG/197-26-06-01-smptest-tool.md
git commit -m "feat(tools): smptest — measure SMP parallel speedup"
```

---

## Task 5: Integration test + VBox verify + roadmap

**Files:**
- Create: `tests/smp2-test.sh`
- Modify: `Makefile` (run-smp2-test)

- [ ] **Step 1: Create `tests/smp2-test.sh`** (boot -smp 4 with disk+net, run smptest over SSH, assert speedup ≥1.5 AND ≥2 distinct cores). Mirror `tests/ssh-shell-test.sh` for the QEMU+SSH harness:
```bash
#!/usr/bin/env bash
# Integration test: SMP Fase 2 — parallel compute pool. Boots -smp 4, runs
# `smptest` over SSH, asserts speedup >= 1.5x and >= 2 distinct cores.
set -u
cd "$(dirname "$0")/.."
KEY=build/id_ed25519; PORT=2222
for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done
sleep 1
for _ in $(seq 1 30); do ss -ltn 2>/dev/null | grep -q ":$PORT " && sleep 1 || break; done
cp "$KEY" /tmp/ruos_id && chmod 600 /tmp/ruos_id
rm -f build/serial-smp2.log build/smptest.log
timeout 60 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -boot d -cdrom build/os.iso \
  -serial stdio -display none -no-reboot -m 256 -device qemu-xhci \
  -netdev user,id=net0,hostfwd=tcp:127.0.0.1:$PORT-:22 \
  -device virtio-net-pci,netdev=net0 \
  -drive file=build/disk.img,format=raw,if=none,id=disk0 \
  -device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0 \
  > build/serial-smp2.log 2>&1 &
QEMUPID=$!
sleep 16
( printf 'smptest\n'; sleep 4; printf 'exit\n'; sleep 1 ) | \
  timeout 25 ssh -tt -p "$PORT" -i /tmp/ruos_id \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    -o ConnectTimeout=5 root@127.0.0.1 > build/smptest.log 2>/dev/null || true
sleep 2
kill "$QEMUPID" 2>/dev/null || true; wait "$QEMUPID" 2>/dev/null || true
echo "=== smptest output ==="; tr -d '\r' < build/smptest.log | grep -iE "parallel=|speedup="
# Extract speedup (Z.ZZ) and core count; pass if speedup >= 1.50 and >=2 cores.
line=$(tr -d '\r' < build/smptest.log | grep -oE 'speedup=[0-9]+\.[0-9]+x cores=\[[0-9,]*\]' | head -1)
spd=$(echo "$line" | grep -oE 'speedup=[0-9]+\.[0-9]+' | grep -oE '[0-9]+\.[0-9]+')
ncores=$(echo "$line" | grep -oE 'cores=\[[0-9,]*\]' | grep -oE '[0-9]+' | sort -u | wc -l)
echo "speedup=$spd distinct_cores=$ncores"
if [ -n "$spd" ] && awk "BEGIN{exit !($spd >= 1.5)}" && [ "${ncores:-0}" -ge 2 ]; then
  echo TEST_PASS_SMP2
else
  echo TEST_FAIL_SMP2; tail -20 build/serial-smp2.log; exit 1
fi
```

- [ ] **Step 2: Makefile target.** After `run-smp-test`, add:
```makefile
.PHONY: run-smp2-test
run-smp2-test: iso ssh-key-on-disk
	bash tests/smp2-test.sh
```
(Match the `ssh-key-on-disk` prereq + TAB style from run-ssh-test.)

- [ ] **Step 3: Run it.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-smp2-test 2>&1 | tail -8'`
Expected: smptest output with speedup ≥1.5 + ≥2 cores, `TEST_PASS_SMP2`. If speedup is below 1.5 on -smp 4 (3 APs), investigate (ITERS too small → timing noise? jobs not parallel?). Tune ITERS if it's a measurement-noise issue, but the speedup must be GENUINE (jobs really ran on ≥2 cores). Do NOT loosen the threshold below a meaningful value to force a pass.

- [ ] **Step 4: Full regression.** Run each (PowerShell, kill qemu between): run-test, run-ssh-test, run-pipe-test, run-fuel-test, run-smp-test. Paste verdicts (all green — BSP executor untouched).

- [ ] **Step 5: VBox verify.** Rebuild (`touch kernel/build.rs && make iso`), boot on VBox (6 vCPU), run smptest over... actually VBox has no SSH hostfwd configured the same way — instead confirm on VBox: banner sha == HEAD, `5/5 APs online`, no #PF, boot to shell. Then (optional but ideal) run smptest manually if you can reach a console; at minimum confirm the kernel with the worker-loop APs boots cleanly on VBox (no #PF from APs now spinning in the pool loop). Paste the VBox markers.
```
$vbm="C:\Program Files\Oracle\VirtualBox\VBoxManage.exe"
Remove-Item "E:\MinimalOS\BasicOperatingSystem\build\log-vbox.log" -EA SilentlyContinue
& $vbm startvm "ruos" --type headless; Start-Sleep 16; & $vbm controlvm "ruos" poweroff; Start-Sleep 2
Get-Content "E:\MinimalOS\BasicOperatingSystem\build\log-vbox.log" | Select-String -Pattern 'ruos v0|APs online|#PF|init.sh complete'
```
Expected: banner sha == HEAD, `5/5 APs online`, `init.sh complete`, NO #PF.

- [ ] **Step 6: Roadmap + CHANGELOG + commit.**
In `docs/superpowers/roadmap-rust-os.md` Step 18 SMP, add "Fase 2 — kernel compute offload pool (✅ DONE)": APs run pure-CPU kernel jobs from an SMP queue in parallel; smptest shows real speedup; no .wasm/STI/preemption on APs; BSP executor unchanged. Note further offload (.wasm on APs, IPI-wake idle, scheduler) remains future. Create `CHANGELOG/198-26-06-01-smp2-test-roadmap.md`. Then:
```bash
git add tests/smp2-test.sh Makefile docs/superpowers/roadmap-rust-os.md CHANGELOG/198-26-06-01-smp2-test-roadmap.md
git commit -m "test(smp): run-smp2-test (parallel speedup) + VBox verify; roadmap Fase 2 done"
```

---

## Self-review notes (addressed)

- **Spec coverage:** pool (T1) ✓; AP worker loop (T2) ✓; bench host fn parallel-vs-sequential + ran_on (T3) ✓; smptest tool (T4) ✓; integration test + VBox + roadmap (T5) ✓. Done-criteria: parallel jobs on ≥2 cores (T5 assert), 1-CPU inline fallback (T3 n_aps==0 drain), no .wasm/STI on APs (T2), BSP executor untouched (no executor edits anywhere), speedup ≥1.5 (T5).
- **Timing source confirmed:** `boot::clock::elapsed_ms()` (TSC, lock-free) — not the 10ms ticks.
- **fn-pointer round-trip:** documented unsafe boundary (transmute usize→JobFn); fallback to AtomicPtr noted if transmute rejected.
- **No-AP fallback:** T3 drains the queue inline on the BSP so the bench never deadlocks on 1 CPU.
- **Job purity invariant:** hash_job is pure-CPU over a 'static const buffer — no I/O, no shared mutable state, zero contention. The whole correctness rests on this; documented.
- **BSP executor untouched:** no task edits kernel/src/executor/ — confirmed by file list.
