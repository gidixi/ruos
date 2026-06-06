# Step 4 (pty-core) — PTY ownership: SPSC slave-input ring + owner-routed stdout Implementation Plan

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development / executing-plans. `- [ ]`.
> ⚠️ MEDIUM-HIGH RISK: touches the interactive terminal data path (SSH + keyboard + app
> stdio). A bug breaks the shell / SSH. `run-test`, `run-ssh-gui-test`, `run-exec-ap-test`
> are the regression gates at EVERY commit.

**Goal:** remove the last cross-core RW-hot lock on the interactive data path. Today every
PTY operation takes `PAIRS[i]: Mutex<PtyPair>` with `without_interrupts` (IF=0). Under
parallel `.cwasm` apps (C2d), an app on core 2 writing stdout takes `pair(n).lock()` on
core 2 while the BSP (`ssh_serve`) reads `master_out` under the same lock → cross-core
contention + IF-off jitter on both cores (delays TLB-shootdown ack / wake / timer). Give
each pair an **owner core** (= BSP for v1); the app-side stdio of an off-owner core no
longer locks the pair.

**Value (honest):** measurable win today is small (few apps, mostly BSP). The real value is
(1) shared-nothing coherence — PTY is the last contended cross-core data-path lock; (2)
less interrupt jitter (no `without_interrupts` on a contended cross-core lock), which the
GUI core benefits from; (3) ready for multi-app interactive workloads (north-star). Spec §9.

**Design (chosen after reading the data flow — NOT the spec's per-byte bus, which would
regress the high-volume stdout path):**
- **Owner = BSP (core 0) for v1.** All PTY input (keyboard ISR + SSH) and SSH master-read
  run on the BSP, so the BSP is the natural owner. `pty_owner(idx) -> u32` returns 0 for
  now; documented extension point for round-robin pty-cores when core count grows (the
  design below stays valid as long as ONE core owns a given pair = single producer of its
  input ring).
- **`slave_rx` → SPSC lock-free byte ring.** Single producer = the owner (line discipline,
  on the owner/BSP). Single consumer = the app-core reading stdin. Fixed capacity (no alloc
  in ISR context — strictly safer than today's `VecDeque` that may grow under the ISR lock).
  App reads lock-free; owner writes lock-free; cross-core `Waker` wake already works
  (`Waker::wake` → `__pender` → targeted IPI, Step 2/3c).
- **App stdout `write` (the high-volume direction) → routed to the owner via the EXISTING
  u64 bus** (`smp::inbox::request`). One message per `write()` syscall (NOT per byte) —
  same granularity as today's one-lock-per-`write()`, so NO throughput regression. The
  owner runs `process_output` locally into its owner-local `master_out`. Reply u64 =
  bytes written.
- **`master_out`, `termios`, `line_buffer`, `master_waker`, echo → owner-local.** Only the
  owner touches them, so they keep a per-pair lock that is NEVER contended cross-core (cheap
  on the owner; no cross-core cache-line bouncing). `foreground_pid` → `AtomicU32` (0 =
  none) so the app-side read path can check it without the pair lock.
- **Net effect:** an app-core never locks `pair(n)`. Reads come from the SPSC ring; writes
  go over the bus to the owner. Zero cross-core PTY lock.

**Discovery already done (context for the implementer — verify if anything looks off):**
- AP `.cwasm` stdout path: `wasm/wt/wasi.rs:43` `fd_write` → `vfs::block_on(vfs::write(pfd,
  bytes))` → `vfs/devices.rs PtySlaveFile::write` → `pty::pair(n).lock()` on the AP. So the
  routing belongs in `PtySlaveFile::write` (it has `idx`; branch on `cpu_id() != owner`).
- `vfs::block_on` (`vfs/block_on.rs`) is a core-agnostic busy-poll — works on the AP. A
  blocking stdin read on an AP busy-polls that AP's run_cwasm (pre-existing C2b/C2c
  behavior; pty-core does NOT change it — the SPSC ring just makes the poll lock-free).
- `master_out` has TWO producers (app stdout via `process_output` + owner echo via
  `process_input`) → it canNOT be a clean SPSC ring; that's WHY it stays owner-local and
  the app's writes are routed to the owner instead.

**Prerequisites (committed):** Step 2 (inbox bus + targeted IPI), 3b/3c (per-core executors,
cross-core wake), 1a fast cpu_id. C2d (parallel apps make this contention real + testable).

**CHANGELOG:** next free on this branch = **320**. Trailer:
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## Task 1: SPSC byte ring for slave input

**Files:** `kernel/src/pty/spsc.rs` (new), `kernel/src/pty/mod.rs` (module decl)

- [ ] **Step 1: ring type** — Create `kernel/src/pty/spsc.rs`: a fixed-capacity
  single-producer/single-consumer byte ring. Power-of-two capacity, `head`/`tail`
  `AtomicUsize`, lock-free. Producer = owner; consumer = app-core. Drops bytes if full
  (terminal input overrun — acceptable, matches a real tty's input flow control absence;
  log once if it ever fills). No allocation after construction (safe in ISR).
```rust
//! Single-producer / single-consumer lock-free byte ring for PTY slave input.
//! Producer = the pair's owner core (line discipline); consumer = the app core
//! reading stdin. Cross-core handoff via head/tail atomics; the consumer's Waker
//! is woken by the producer (Waker::wake is cross-core-safe via __pender).
use core::sync::atomic::{AtomicUsize, Ordering};
use core::cell::UnsafeCell;

const CAP: usize = 4096;              // power of two; ample for tty input
const MASK: usize = CAP - 1;

pub struct SpscRing {
    buf: UnsafeCell<[u8; CAP]>,
    head: AtomicUsize,                // next write index (producer)
    tail: AtomicUsize,                // next read index (consumer)
}
// SAFETY: exactly one producer core touches `head` + buf[head..]; exactly one
// consumer core touches `tail` + buf[tail..]. The atomics order the handoff.
unsafe impl Sync for SpscRing {}

impl SpscRing {
    pub const fn new() -> Self {
        SpscRing {
            buf: UnsafeCell::new([0; CAP]),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }
    /// Producer: push one byte. Returns false if full (byte dropped).
    pub fn push(&self, b: u8) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= CAP { return false; } // full
        // SAFETY: producer-exclusive slot.
        unsafe { (*self.buf.get())[head & MASK] = b; }
        self.head.store(head.wrapping_add(1), Ordering::Release);
        true
    }
    /// Consumer: pop one byte, or None if empty.
    pub fn pop(&self) -> Option<u8> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail == head { return None; } // empty
        // SAFETY: consumer-exclusive slot.
        let b = unsafe { (*self.buf.get())[tail & MASK] };
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(b)
    }
    pub fn is_empty(&self) -> bool {
        self.tail.load(Ordering::Acquire) == self.head.load(Ordering::Acquire)
    }
}
```

- [ ] **Step 2: module decl** — In `kernel/src/pty/mod.rs`, add `pub mod spsc;` next to the
  other `pub mod` lines (after `pub mod pair;`).

- [ ] **Step 3: build** — `wsl ... make iso`. Expected clean (type compiles, unused for now).

- [ ] **Step 4: commit** —
```
git add kernel/src/pty/spsc.rs kernel/src/pty/mod.rs
git commit -m "feat(smp): Step 4 pty — SPSC lock-free byte ring for slave input"
```

## Task 2: pair ownership + SPSC slave_rx + owner-routed stdout

**Files:** `kernel/src/pty/pair.rs`, `kernel/src/pty/mod.rs`, `kernel/src/pty/ldisc.rs`,
`kernel/src/vfs/devices.rs`, `kernel/src/executor/mod.rs` (owner-op fns + bus glue)

- [ ] **Step 0: re-read the touch points** — `ldisc.rs` (`process_input` pushes to
  `slave_rx`; `process_output` + echo push to `master_out`), `pty/mod.rs`
  (`master_input_push`, `slave_read_one_timeout`, `master_output_*`, `request_shutdown`,
  `set_foreground`), `vfs/devices.rs` (`PtySlaveFile::read/write`). Confirm every
  `g.slave_rx` access (producer in ldisc; consumer in mod.rs + devices.rs) and every
  `g.foreground_pid` access. These are the call sites to migrate.

- [ ] **Step 1: `pty_owner` + move `slave_rx` to the ring + `foreground_pid` atomic** —
  In `pty/mod.rs`:
```rust
/// Owner core for pair `idx`. v1: the BSP owns every pair (all PTY input + SSH
/// master-read run on the BSP). Extension point: round-robin to dedicated
/// pty-cores when core count grows — the SPSC ring stays valid as long as ONE
/// core (the owner) is the sole producer of `idx`'s slave input.
pub fn pty_owner(_idx: usize) -> u32 { 0 }

/// Per-pair slave-input ring (replaces `PtyPair::slave_rx`). Producer = owner
/// (line discipline); consumer = the app core reading stdin.
static SLAVE_RX: [spsc::SpscRing; NUM_PAIRS] = [
    spsc::SpscRing::new(), spsc::SpscRing::new(),
    spsc::SpscRing::new(), spsc::SpscRing::new(),
];

/// Foreground pid per pair as an atomic (0 = none) so the app-side read path
/// can check kill/EOF without taking the owner-local pair lock.
static FOREGROUND: [core::sync::atomic::AtomicU32; NUM_PAIRS] = [
    core::sync::atomic::AtomicU32::new(0), core::sync::atomic::AtomicU32::new(0),
    core::sync::atomic::AtomicU32::new(0), core::sync::atomic::AtomicU32::new(0),
];
pub fn slave_rx_ring(idx: usize) -> &'static spsc::SpscRing { &SLAVE_RX[idx] }
```
  - Remove `slave_rx` (and the unused `slave_tx`, `master_in` if confirmed unused — grep
    first) from `PtyPair` in `pair.rs`. Keep `master_out`, `line_buffer`, `termios`,
    `master_waker`, `slave_waker`.
  - `set_foreground(idx, pid)` → `FOREGROUND[idx].store(pid.unwrap_or(0), SeqCst)` (drop the
    pair-lock version). Add `pub fn foreground_pid(idx) -> Option<u32>` reading the atomic.

- [ ] **Step 2: producer side — `ldisc::process_input` pushes to the ring** — In `ldisc.rs`,
  replace `pair.slave_rx.push_back(byte)` (and the other `slave_rx.push_back`) with
  `crate::pty::slave_rx_ring(idx).push(byte)`. This needs `idx` in `process_input` — change
  its signature to take `idx: usize` (the caller `master_input_push` has it). The echo
  writes to `master_out` stay as-is (owner-local). Update `process_input`'s callers.
  > `master_input_push` runs on the owner (BSP keyboard ISR / SSH) → the ring producer is
  > the owner. After pushing, wake the slave consumer's waker (see Step 4).

- [ ] **Step 3: consumer side — reads from the ring, kill/EOF via the atomic** — In
  `pty/mod.rs::slave_read_one_timeout` and `vfs/devices.rs::PtySlaveFile::read`: replace
  `g.slave_rx.pop_front()` with `crate::pty::slave_rx_ring(idx).pop()` (NO pair lock for the
  data). Replace the `g.foreground_pid.map(...)` kill check with
  `crate::pty::foreground_pid(idx).map(|p| crate::proc::is_kill_pending(p)).unwrap_or(false)`.
  Register the consumer waker via a dedicated cross-core slot (Step 4) — NOT under the pair
  lock. EOF/shutdown via `is_shutdown(idx)` (already an atomic).

- [ ] **Step 4: slave waker as a cross-core slot** — The consumer (app-core) registers its
  `Waker`; the producer (owner) wakes it after `push`. Move `slave_waker` out of the pair
  lock into an `IrqMutex<Option<Waker>>` array (so producer + consumer don't share the pair
  lock just for the waker):
```rust
static SLAVE_WAKER: [crate::sync::IrqMutex<Option<core::task::Waker>>; NUM_PAIRS] =
    [crate::sync::IrqMutex::new(None), crate::sync::IrqMutex::new(None),
     crate::sync::IrqMutex::new(None), crate::sync::IrqMutex::new(None)];
pub fn register_slave_waker(idx: usize, w: core::task::Waker) { *SLAVE_WAKER[idx].lock() = Some(w); }
pub fn wake_slave(idx: usize) { if let Some(w) = SLAVE_WAKER[idx].lock().take() { w.wake(); } }
```
  Call `wake_slave(idx)` at the end of `master_input_push` (after the ring push) and in
  `request_shutdown`. Consumers call `register_slave_waker` instead of `g.slave_waker = ...`.

- [ ] **Step 5: owner-routed stdout write** — In `vfs/devices.rs::PtySlaveFile::write`:
```rust
async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> {
    if buf.is_empty() { return Ok(0); }
    let idx = self.idx;
    let owner = crate::pty::pty_owner(idx);
    if crate::cpu::cpu_id() == owner {
        // Local fast path (owner core): process_output into owner-local master_out.
        x86_64::instructions::interrupts::without_interrupts(|| {
            let mut g = crate::pty::pair(idx).lock();
            for &b in buf { crate::pty::ldisc::process_output(&mut g, b); }
        });
    } else {
        // Off-owner (e.g. .cwasm app on core 2): route the whole slice to the
        // owner via the bus (one message per write() — same granularity as the
        // local lock path). Owner runs process_output locally.
        let n = crate::pty::route_write_to_owner(idx, buf).await;
        crate::pty::touch_activity(idx);
        return Ok(n);
    }
    crate::pty::touch_activity(idx);
    Ok(buf.len())
}
```
  In `pty/mod.rs`, the bus glue (encode idx + bytes into the message input; owner-side op
  decodes, locks the pair locally, runs `process_output`, returns nwritten):
```rust
/// Off-owner caller: send `buf` to pair `idx`'s owner; the owner appends it to
/// master_out via process_output. Returns bytes accepted. One bus msg per call.
pub async fn route_write_to_owner(idx: usize, buf: &[u8]) -> usize {
    let owner = pty_owner(idx);
    let mut input = alloc::vec::Vec::with_capacity(4 + buf.len());
    input.extend_from_slice(&(idx as u32).to_le_bytes());
    input.extend_from_slice(buf);
    let n = crate::smp::inbox::request(owner, pty_write_op, input.into_boxed_slice()).await;
    n as usize
}

/// Owner-side bus op: input = [idx:u32 le][bytes...]; run process_output locally.
fn pty_write_op(input: &[u8]) -> u64 {
    if input.len() < 4 { return 0; }
    let idx = u32::from_le_bytes([input[0], input[1], input[2], input[3]]) as usize;
    if idx >= NUM_PAIRS { return 0; }
    let bytes = &input[4..];
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut g = PAIRS[idx].lock();
        for &b in bytes { ldisc::process_output(&mut g, b); }
    });
    bytes.len() as u64
}
```
  > The owner runs `pty_write_op` when it drains its inbox (`drain_inbox`, already wired in
  > the BSP poll loop). The pair lock is taken ONLY on the owner → never contended
  > cross-core. `request(...)` lives in `smp::inbox` (Step 2). `pty_write_op` is a plain
  > `fn` (no captures) → valid as the bus op pointer.

- [ ] **Step 6: build (1 core)** — `wsl ... make iso`. On 1 core `cpu_id()==owner(=0)`
  always → local path → today's behavior. Expected clean.

- [ ] **Step 7: regression (1 core, CRITICAL)** — `wsl ... make run-test` → `TEST_PASS`.
  Exec + shell stdio exercised. A hang/garbled output ⇒ the ring or waker wiring is wrong.

- [ ] **Step 8: commit** —
```
git add kernel/src/pty/pair.rs kernel/src/pty/mod.rs kernel/src/pty/ldisc.rs kernel/src/vfs/devices.rs kernel/src/executor/mod.rs
git commit -m "feat(smp): Step 4 pty — owner-routed stdout + SPSC slave-input (app core never locks the pair)"
```

## Task 3: gate — parallel SSH apps, stdio correct + routed, no regression

**Files:** `kernel/src/boot/phases/interrupts.rs` (boot-check marker), `Makefile` (maybe a
2-session test), `CHANGELOG/320-...`

- [ ] **Step 1: routed-marker boot-check (optional but decisive)** — Under `boot-checks`,
  add a check that proves an off-owner write routes: on -smp 4, `spawn_on(2, ...)` a tiny
  task that calls `route_write_to_owner(<unclaimed test idx>, b"PTYROUTE")` and asserts the
  owner (core 0) processed it (e.g. the bus op increments a `static AtomicU32 PTY_ROUTED`,
  and the task confirms it ran from core 2 via `cpu_id()`), log:
  `crate::binfo!("pty-route", "from=core{} routed_ok={}", ran, ok);`. Gate: `from=core2
  routed_ok=true`. (Pick a pair idx not used by the SSH/console path, or use master_output
  drain to verify.)

- [ ] **Step 2: THE behavioral gate — 2 concurrent SSH `.cwasm` apps** — Reuse/extend
  `run-exec-ap-test` or `run-smp2-test`: open TWO SSH sessions, each `exec` a `.cwasm` tool
  that prints a distinct marker (e.g. `wtecho SESSION_A` / `wtecho SESSION_B`), assert BOTH
  markers reach their respective clients (stdout correct cross-core via the routed path) AND
  the routed apps ran on ComputeApp cores. If a 2-session harness is too heavy, the existing
  `run-exec-ap-test` (one `.cwasm` on core 2 → its stdout reaches the terminal) already
  exercises the routed-write path end to end — assert `EXEC_AP_OK` still appears (now via
  the bus route, not the local lock).

- [ ] **Step 3: full regression suite + parallel gate, build boot-checks + -smp 4** —
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso CARGO_FEATURES="boot-checks" && timeout 120 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -m 512 -no-reboot -display none -serial stdio -device qemu-xhci -cdrom build/os.iso 2>&1 | grep -E "pty-route|parallel-exec|#DF|#PF|panic"'
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test && make run-exec-ap-test && make run-ssh-gui-test && make run-smp-test && make run-smp2-test'
```
  GATES (ALL must hold): `pty-route from=core2 routed_ok=true`; `TEST_PASS`;
  `TEST_PASS_EXEC_AP` (`EXEC_AP_OK` via the routed write); `TEST_PASS_SSH`; `TEST_PASS_SMP`;
  `TEST_PASS_SMP2`. The C2d `parallel-exec ... overlap=true` must STILL hold (pty-core
  didn't touch the exec path). NO `#DF`/`#PF`/`panic`.
  > Report RAW marker lines, not "passed". SSH/keyboard timing is QEMU-flaky → re-run any
  > SSH gate that fails once before treating it as real.

- [ ] **Step 4: changelog + commit** — `CHANGELOG/320-26-06-07-step4-pty-core.md`: SPSC
  slave-input ring, owner=BSP routing of app stdout via the bus, foreground/waker moved out
  of the pair lock, the "app core never locks the pair" result + the jitter rationale. Then:
```
git add kernel/src/boot/phases/interrupts.rs Makefile kernel/... CHANGELOG/320-26-06-07-step4-pty-core.md
git commit -m "test(smp): Step 4 pty — routed-write gate + 2 concurrent SSH apps stdio"
```

---

## Self-Review / risks
- **No throughput regression:** writes route ONE bus message per `write()` syscall — the
  same granularity as today's one-lock-per-`write()`. Reads are lock-free (SPSC ring), no
  bus. The high-volume path (stdout) is owner-local after one hop; the low-volume path
  (stdin) is lock-free.
- **SPSC invariant:** exactly one producer (the owner) and one consumer (the app-core) per
  pair. Holds for owner=BSP (all input on BSP). If round-robin pty-cores are added later,
  the owner is still the sole producer → invariant preserved. Do NOT let two cores produce
  into the same ring.
- **ISR safety:** the ring `push` is lock-free + alloc-free → safe in the keyboard ISR
  (today's `VecDeque` under a lock could allocate in ISR — the ring is strictly safer).
- **Waker races:** register the consumer waker BEFORE the final emptiness re-check (the
  existing poll_fn pattern). The producer wakes after `push` + `Release`. Standard SPSC +
  waker handoff; mirror the existing `ReplyFuture` waker discipline.
- **Lock ordering:** the owner-side `pty_write_op` takes ONLY the pair lock (no registry,
  no other lock) → no new ordering hazard. `master_input_push` already drops the pair lock
  before `request_kill` (registry) — keep that.
- **`block_on` on AP:** unchanged; a blocking stdin read still busy-polls the AP (pre-
  existing). pty-core only makes that poll lock-free. Not a regression.
- **Biggest risk:** the `slave_rx`→ring migration touches the shell's stdin path. `run-test`
  (shell execs) + `run-ssh-gui-test` (interactive SSH) are the gates — keep them green at
  Task 2's commit, not just at the end.
