# Step 2 — Inter-core message bus + cross-core wake Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or executing-plans. Steps use `- [ ]` checkboxes.

**Goal:** Build the inter-core message bus (per-core inbox + targeted IPI + async
request/reply) AND the single cross-core wake primitive (`__pender` per-core, targeted
IPI) that Steps 3/4/5 all depend on. Spec: `2026-06-05-smp-shared-nothing-migration-design.md`
§3 (foundation: cross-core wake) + §7 (message bus).

**Architecture:** Each core owns an inbox (`IrqMutex<VecDeque<Box<InboxMsg>>>`). A sender
on core A `core_send(B, msg)` → enqueues to B's inbox + fires a TARGETED IPI (`VEC_INBOX`)
to B. B's loop (AP worker loop / BSP poll loop) drains its inbox, runs each message's
`op(input)`, publishes the result, and wakes the sender's `Waker`. The sender `await`s a
`ReplyFuture` that resolves when the result is published. The cross-core wake is the
general primitive: `WAKE_PENDING` becomes a per-core array; `__pender` reads the OWNER
core id from the executor's context pointer and, if signalled from a different core,
fires a targeted `VEC_WAKE` IPI so the owner leaves `hlt`. The BSP executor is created
with its owner id (0) encoded in the context pointer.

**Tech Stack:** Rust `no_std`, `alloc` (Box/Arc), `IrqMutex`, embassy `RawExecutor`/
`Waker`, LAPIC ICR (targeted IPI), magazine allocator (Step 1b — handles cross-core free
of the Box/Arc), RDTSCP `cpu_id()` (Step 1a). Boot-check markers for tests.

**Prerequisites (committed):** Step 1a (fast cpu_id) + Step 1b (magazine default — the
InboxMsg Box is allocated on the sender's core and dropped on the receiver's; the
magazine recycles cross-core-freed blocks correctly, so no remote-free queue is needed).

**Why this is testable standalone (before Step 3):** Only the BSP runs an executor today.
The bus is exercised by a boot-check: the BSP sends a message to an AP, the AP (in its
existing `ap_worker_loop`) drains + runs it + wakes, and the BSP `await`s the reply. The
AP→BSP wake is the general `__pender` primitive with owner=0. Step 3 later adds AP
executors with their own owner ids; the same primitive works unchanged.

---

## File Structure

- `kernel/src/apic/lapic.rs` — add `send_ipi(lapic_id, vector)` (physical-dest targeted
  IPI), keep `send_ipi_all_but_self`.
- `kernel/src/idt.rs` — add `VEC_INBOX = 0x41` + `inbox_handler` (set this core's
  INBOX_PENDING + eoi). Reserve `VEC_TLB_SHOOTDOWN = 0x42`, `VEC_RESET = 0x43` as
  constants (no handlers yet — Steps 3/6) so the vector budget is documented.
- `kernel/src/smp/inbox.rs` — NEW. `InboxMsg`, `ReplySlot`, `PER_CORE_INBOX`,
  `INBOX_PENDING`, `core_send`, `core_recv`, `drain_inbox`, `ReplyFuture`.
- `kernel/src/smp/mod.rs` — `pub mod inbox;`.
- `kernel/src/executor/mod.rs` — `WAKE_PENDING` → `[AtomicBool; MAX_CPUS]`; `__pender`
  reads owner from context + targeted IPI; `run()` creates the executor with owner id 0
  in the context, uses `WAKE_PENDING[0]`, and drains the inbox each loop; add a
  `wake_core(owner)` helper.
- `kernel/src/cpu/ap.rs` — AP worker loop drains its inbox + checks INBOX_PENDING before
  halting.
- `kernel/src/boot/phases/interrupts.rs` — boot-check: BSP→AP message round-trip.
- `CHANGELOG/NN-...` (next free on this branch; currently 306 is highest → 307).

**Vector budget (documented in idt.rs):** `0x20` timer, `0x21` kbd, `0x22` mouse,
`0x40` WAKE, `0x41` INBOX, `0x42` TLB_SHOOTDOWN (reserved, Step 3), `0x43` RESET
(reserved, Step 6), `0xFF` spurious.

---

## Task 1: Targeted IPI + VEC_INBOX vector & handler

**Files:** `kernel/src/apic/lapic.rs`, `kernel/src/idt.rs`

- [ ] **Step 1: targeted IPI in lapic.rs** — Add below `send_ipi_all_but_self`:
```rust
/// Send a fixed IPI with `vector` to a SINGLE target core, addressed by its xAPIC
/// `lapic_id` (physical destination mode). Used for targeted cross-core wake
/// (VEC_WAKE) and inbox delivery (VEC_INBOX).
pub fn send_ipi(lapic_id: u32, vector: u8) {
    let low = ICR_DELIVERY_FIXED | ICR_LEVEL_ASSERT | vector as u32; // dest shorthand = 0 (no shorthand) → physical
    unsafe {
        write_volatile(reg(REG_ICR_HIGH), lapic_id << 24); // dest in bits 24-31 (xAPIC)
        write_volatile(reg(REG_ICR_LOW), low);
    }
}
```
Verify the existing ICR constants: `ICR_DELIVERY_FIXED`, `ICR_LEVEL_ASSERT` exist (used
by `send_ipi_all_but_self`). The dest-shorthand field (bits 18-19) is 0 here = "no
shorthand" = use the destination field in ICR_HIGH. Confirm `ICR_DEST_ALL_BUT_SELF` is
a shorthand bitmask so omitting it yields shorthand 0 (physical). If the existing
constant layout differs, match it — READ the current `send_ipi_all_but_self` + the const
definitions first.

- [ ] **Step 2: VEC_INBOX + reserved vectors in idt.rs** — After `VEC_WAKE`:
```rust
pub const VEC_INBOX:         u8 = 0x41;
/// Reserved (Step 3 — cross-core TLB shootdown). No handler yet.
pub const VEC_TLB_SHOOTDOWN: u8 = 0x42;
/// Reserved (Step 6 — supervisor core reset). No handler yet.
pub const VEC_RESET:         u8 = 0x43;
```
Register the inbox handler in `init()` (next to `idt[VEC_WAKE]...`):
```rust
idt[VEC_INBOX].set_handler_fn(inbox_handler);
```
Add the handler (next to `wake_handler`):
```rust
/// Inbox-delivery IPI handler. Marks this core's inbox as pending so its loop
/// (executor poll / AP worker) drains it, then EOIs.
extern "x86-interrupt" fn inbox_handler(_frame: InterruptStackFrame) {
    crate::smp::inbox::mark_pending(crate::cpu::cpu_id());
    crate::apic::lapic::eoi();
}
```
(`smp::inbox::mark_pending` is added in Task 2; this won't compile until then — do Task 2
next before building.)

- [ ] **Step 3: (no build yet — depends on Task 2). Commit deferred to Task 2.**

---

## Task 2: Per-core inbox + async request/reply (`smp/inbox.rs`)

**Files:** Create `kernel/src/smp/inbox.rs`; modify `kernel/src/smp/mod.rs`.

- [ ] **Step 1: smp/mod.rs** — add `pub mod inbox;`.

- [ ] **Step 2: write `smp/inbox.rs`** —
```rust
//! Inter-core message bus: per-core inbox + targeted IPI + async request/reply.
//! A sender on core A posts a message to core B's inbox and IPIs B; B drains its
//! inbox in its run loop, runs the message op, publishes the result, and wakes the
//! sender's Waker. No shared mutable state crosses the boundary — only the owned
//! message (Box) and the reply slot (Arc). The Box is allocated on A and dropped on
//! B (cross-core free) — the magazine allocator (Step 1b) handles that.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use core::task::{Context, Poll, Waker};
use core::future::Future;
use core::pin::Pin;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::collections::VecDeque;
use crate::sync::IrqMutex;
use crate::cpu::MAX_CPUS;

/// Result channel for one request. Shared (Arc) between sender and the executing core.
pub struct ReplySlot {
    result: AtomicU64,
    done: AtomicBool,
    waker: IrqMutex<Option<Waker>>,
}
impl ReplySlot {
    fn new() -> Self {
        Self { result: AtomicU64::new(0), done: AtomicBool::new(false), waker: IrqMutex::new(None) }
    }
    fn complete(&self, value: u64) {
        self.result.store(value, Ordering::SeqCst);
        self.done.store(true, Ordering::SeqCst);                 // Release of the result
        if let Some(w) = self.waker.lock().take() { w.wake(); }  // cross-core wake
    }
}

/// A message to run an op on the target core. `op` is a pure fn (like JobFn) but the
/// reply is delivered asynchronously via the ReplySlot + the sender's Waker.
pub struct InboxMsg {
    op: fn(&[u8]) -> u64,
    input: Box<[u8]>,
    reply: Arc<ReplySlot>,
}

struct CoreInbox {
    queue: IrqMutex<VecDeque<Box<InboxMsg>>>,
    pending: AtomicBool,
}
impl CoreInbox {
    const fn new() -> Self { Self { queue: IrqMutex::new(VecDeque::new()), pending: AtomicBool::new(false) } }
}

static PER_CORE_INBOX: [CoreInbox; MAX_CPUS] = {
    const I: CoreInbox = CoreInbox::new();
    [I; MAX_CPUS]
};

/// IPI handler hook: mark `cpu`'s inbox pending (called from inbox_handler).
pub fn mark_pending(cpu: u32) {
    PER_CORE_INBOX[cpu as usize].pending.store(true, Ordering::SeqCst);
}

/// Post `msg` to `target`'s inbox and IPI it. SeqCst publish before the IPI so the
/// target observes the enqueue when it drains (the IPI is the wake, not the fence —
/// but the IrqMutex enqueue + SeqCst pending flag order the handoff).
fn enqueue(target: u32, msg: Box<InboxMsg>) {
    PER_CORE_INBOX[target as usize].queue.lock().push_back(msg);
    PER_CORE_INBOX[target as usize].pending.store(true, Ordering::SeqCst);
    let lapic = crate::cpu::lapic_id_of(target);
    crate::apic::lapic::send_ipi(lapic, crate::idt::VEC_INBOX);
}

/// Async: run `op(input)` on `target` core, await the u64 result.
pub fn request(target: u32, op: fn(&[u8]) -> u64, input: Box<[u8]>) -> ReplyFuture {
    let reply = Arc::new(ReplySlot::new());
    enqueue(target, Box::new(InboxMsg { op, input, reply: reply.clone() }));
    ReplyFuture { reply }
}

pub struct ReplyFuture { reply: Arc<ReplySlot> }
impl Future for ReplyFuture {
    type Output = u64;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u64> {
        if self.reply.done.load(Ordering::SeqCst) {
            Poll::Ready(self.reply.result.load(Ordering::SeqCst))
        } else {
            // Register/refresh the waker BEFORE re-checking done (avoid lost wake).
            *self.reply.waker.lock() = Some(cx.waker().clone());
            if self.reply.done.load(Ordering::SeqCst) {
                Poll::Ready(self.reply.result.load(Ordering::SeqCst))
            } else {
                Poll::Pending
            }
        }
    }
}

/// Drain THIS core's inbox: run each queued message's op and complete its reply.
/// Called from the core's run loop (BSP poll loop / AP worker loop) when pending.
/// Returns the number drained.
pub fn drain_inbox(cpu: u32) -> usize {
    let inbox = &PER_CORE_INBOX[cpu as usize];
    if !inbox.pending.swap(false, Ordering::SeqCst) { return 0; }
    let mut n = 0;
    loop {
        let msg = { inbox.queue.lock().pop_front() };   // drop the lock before running op
        match msg {
            Some(m) => {
                let value = (m.op)(&m.input);
                m.reply.complete(value);                 // wakes the sender (cross-core)
                n += 1;
            }
            None => break,
        }
    }
    n
}

/// True if this core has inbox work pending (for the halt decision).
pub fn is_pending(cpu: u32) -> bool {
    PER_CORE_INBOX[cpu as usize].pending.load(Ordering::SeqCst)
}
```
IMPORTANT: `drain_inbox` MUST NOT hold the queue lock while running `op` or calling
`complete` (which may `wake()` → `__pender` → IPI). The inner `{ ... pop_front() }`
block drops the guard first — keep that structure.

- [ ] **Step 3: `lapic_id_of` in cpu/mod.rs** — `enqueue` needs the target's xAPIC id.
  Add to `cpu/mod.rs`:
```rust
/// xAPIC id of dense core `cpu` (for targeted IPIs). Reads PER_CPU[cpu].lapic_id.
pub fn lapic_id_of(cpu: u32) -> u32 {
    // SAFETY: PER_CPU[cpu] was filled at bring-up before that core ran; lapic_id is
    // written once and read-only after. cpu < MAX_CPUS by construction.
    unsafe { (*core::ptr::addr_of!(PER_CPU.0[cpu as usize])).lapic_id }
}
```

- [ ] **Step 4: build** — `make test-boot` (default). Now Task 1 + Task 2 compile
  together. Expected `TEST_BOOT_PASS` (nothing uses the bus yet — just compiles + the
  IDT has the inbox handler). If it fails, fix before continuing.

- [ ] **Step 5: commit** —
```
git add kernel/src/apic/lapic.rs kernel/src/idt.rs kernel/src/smp/inbox.rs kernel/src/smp/mod.rs kernel/src/cpu/mod.rs
git commit -m "feat(smp): inter-core message bus — per-core inbox + targeted IPI (Step 2 part 1)"
```
Trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## Task 3: Cross-core wake primitive (`__pender` per-core + targeted IPI)

**Files:** `kernel/src/executor/mod.rs`

- [ ] **Step 1: per-core WAKE_PENDING** — Replace `static WAKE_PENDING: AtomicBool` with:
```rust
/// Per-core wake flag. Index = owner core id. Set by `__pender`, cleared by that
/// core's run loop before each poll. (Was a single global AtomicBool — single-core.)
static WAKE_PENDING: [AtomicBool; crate::cpu::MAX_CPUS] = {
    const F: AtomicBool = AtomicBool::new(true);
    [F; crate::cpu::MAX_CPUS]
};
```

- [ ] **Step 2: per-core `__pender` + `wake_core`** — Replace `__pender`:
```rust
/// Wake the owner core: set its WAKE_PENDING and, if we are NOT that core, send it a
/// targeted VEC_WAKE IPI so it leaves `hlt`. Used by `__pender` and any cross-core
/// signaller. ISR-safe: atomic store + (maybe) one IPI write, no locks/allocs.
pub fn wake_core(owner: u32) {
    WAKE_PENDING[owner as usize].store(true, Ordering::SeqCst);
    if crate::cpu::cpu_id() != owner {
        crate::apic::lapic::send_ipi(crate::cpu::lapic_id_of(owner), crate::idt::VEC_WAKE);
    }
}

/// Embassy's pender. `context` carries the OWNER core id (encoded as a pointer when
/// the executor was created). Wakes that core — including a cross-core IPI if the
/// wake originated on a different core (e.g. an AP completing a message reply).
#[no_mangle]
extern "Rust" fn __pender(context: *mut ()) {
    wake_core(context as usize as u32);
}
```

- [ ] **Step 3: BSP executor owner id + run-loop inbox drain** — In `run()`:
  - Create the executor with owner id 0 in the context:
    `slot.write(RawExecutor::new(0usize as *mut ()))` (was `core::ptr::null_mut()` — 0
    is the BSP owner id; null == 0 so behaviorally identical but now meaningful).
  - In the loop, use this core's wake flag and drain its inbox. Replace the
    `WAKE_PENDING.store(false,...)` / `WAKE_PENDING.load(...)` with `WAKE_PENDING[0]`
    (the BSP is core 0), AND after `exec.poll()` add an inbox drain so BSP-targeted
    replies/requests run:
```rust
        WAKE_PENDING[0].store(false, Ordering::SeqCst);
        let poll_start = crate::boot::clock::read_tsc();
        unsafe { exec.poll(); }
        crate::smp::inbox::drain_inbox(0);   // run any messages addressed to the BSP
        crate::sched::cpustat::add_busy(0, crate::boot::clock::read_tsc().saturating_sub(poll_start));

        interrupts::disable();
        // Halt only if no wake AND no inbox work pending (avoid missed inbox wake).
        if WAKE_PENDING[0].load(Ordering::SeqCst) || crate::smp::inbox::is_pending(0) {
            interrupts::enable();
        } else {
            let hlt_start = crate::boot::clock::read_tsc();
            interrupts::enable_and_hlt();
            crate::sched::cpustat::add_idle(0, crate::boot::clock::read_tsc().saturating_sub(hlt_start));
        }
```

- [ ] **Step 4: build** — `make test-boot`. Expected `TEST_BOOT_PASS` (executor still
  single BSP, now per-core wake + inbox-aware, no behavior change observable yet).

- [ ] **Step 5: commit** —
```
git add kernel/src/executor/mod.rs
git commit -m "feat(smp): cross-core wake primitive — per-core __pender + targeted IPI (Step 2 part 2)"
```
Trailer as above.

---

## Task 4: AP inbox drain + the end-to-end round-trip boot-check

**Files:** `kernel/src/cpu/ap.rs`, `kernel/src/boot/phases/interrupts.rs`

- [ ] **Step 1: AP worker loop drains its inbox** — In `cpu/ap.rs ap_worker_loop`, after
  the pool-drain `while let Some(slot)...` block and before the halt decision, add an
  inbox drain, and include inbox-pending in the halt condition:
```rust
        // Run any inter-core messages addressed to this core.
        crate::smp::inbox::drain_inbox(me as u32);

        x86_64::instructions::interrupts::disable();
        if crate::smp::pool::is_empty() && !crate::smp::inbox::is_pending(me as u32) {
            let idle_start = crate::boot::clock::read_tsc();
            x86_64::instructions::interrupts::enable_and_hlt();
            crate::sched::cpustat::add_idle(me, crate::boot::clock::read_tsc().saturating_sub(idle_start));
        } else {
            x86_64::instructions::interrupts::enable();
        }
```
(Read the current loop first; preserve its existing structure/accounting, just add the
inbox drain + the `&& !is_pending` term.)

- [ ] **Step 2: write the boot-check round-trip** — In `boot/phases/interrupts.rs`,
  inside a `#[cfg(feature = "boot-checks")]` block AFTER `smp::bringup()` (alongside the
  cpuprobe / allocbench calls), add a test that sends a message to an AP and awaits the
  reply on the BSP. Since the BSP executor isn't running yet at this boot phase, drive
  the future to completion with a tiny inline poll loop (the BSP also drains its own
  inbox; the AP drains via its worker loop):
```rust
    #[cfg(feature = "boot-checks")]
    {
        if crate::cpu::cpus_online() >= 2 {
            // op: sum the input bytes; run it on core 1.
            fn sum_op(input: &[u8]) -> u64 { input.iter().map(|&b| b as u64).sum() }
            let input: alloc::boxed::Box<[u8]> = alloc::boxed::Box::from(&[1u8, 2, 3, 4][..]);
            let mut fut = crate::smp::inbox::request(1, sum_op, input);
            // Drive the future inline (no executor in this phase). A no-op waker is
            // fine — we poll in a bounded spin; the AP completes the reply async.
            use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
            fn noop(_: *const ()) {}
            fn clone(_: *const ()) -> RawWaker { RawWaker::new(core::ptr::null(), &VT) }
            static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
            let waker = unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VT)) };
            let mut cx = Context::from_waker(&waker);
            let mut result = None;
            for _ in 0..50_000_000u64 {
                if let Poll::Ready(v) = core::future::Future::poll(core::pin::Pin::new(&mut fut), &mut cx) {
                    result = Some(v); break;
                }
                core::hint::spin_loop();
            }
            match result {
                Some(v) => crate::binfo!("inbox", "roundtrip ok core1 sum={} (expect 10)", v),
                None => crate::binfo!("inbox", "roundtrip TIMEOUT"),
            }
        } else {
            crate::binfo!("inbox", "roundtrip skipped (1 core)");
        }
    }
```
This proves: targeted IPI delivery (BSP→AP1), AP inbox drain + op execution, reply
publish, and the future resolving on the BSP. (The cross-core `__pender` wake is
exercised once Step 3 runs the BSP executor; here we poll inline.)

- [ ] **Step 3: build + run with -smp 4** — Default test-boot uses 1 core (the check
  logs "skipped"). Run with APs to exercise it:
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso CARGO_FEATURES="boot-checks" && timeout 90 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -m 512 -no-reboot -display none -serial stdio -device qemu-xhci -cdrom build/os.iso 2>&1 | grep -E "inbox|APs online"'
```
Expected: `inbox roundtrip ok core1 sum=10 (expect 10)`. If TIMEOUT, the IPI/inbox path
is broken — debug (check VEC_INBOX registered, send_ipi dest encoding, AP drains inbox).
Also run plain `make test-boot` (1 core) → expects `inbox roundtrip skipped (1 core)` +
`TEST_BOOT_PASS`.

- [ ] **Step 4: full regression** — `make test-boot` (PASS) + `make run-smp-test` +
  `make run-smp2-test` (the AP path changed — confirm the compute pool + SMP still work).

- [ ] **Step 5: changelog + commit** — next number (verify: highest on branch +1). Cosa:
  Step 2 message bus + cross-core wake; boot-check round-trip BSP→AP sum=10. Perché: the
  foundation Steps 3/4/5 need (cross-core wake + ownership-by-message).
```
git add kernel/src/cpu/ap.rs kernel/src/boot/phases/interrupts.rs CHANGELOG/NN-...
git commit -m "test(smp): Step 2 — inbox round-trip boot-check + AP inbox drain"
```
Trailer as above.

---

## Self-Review

**Spec coverage (§3 + §7):** per-core inbox (Task 2), targeted IPI (Task 1), async
request/reply with waker (Task 2), VEC_INBOX + vector budget (Task 1), cross-core wake
`__pender` per-core + targeted IPI (Task 3, the single primitive §3 mandates), inbox
drain in both run loops (Tasks 3+4). Waker Send/Sync: the reply waker is stored in an
`IrqMutex<Option<Waker>>` and `.wake()`d from the executing core — it only calls
`__pender` (set flag + IPI), never touches the remote executor's run-queue, so it is
safe by construction (§3) — verified by the round-trip test resolving correctly.

**Cross-core free (§13 risk):** the `InboxMsg` Box + the `Arc<ReplySlot>` are allocated
on the sender and dropped on the receiver / when the last Arc ref drops on either core.
The magazine allocator (Step 1b) recycles cross-core-freed blocks (to the freeing core's
cache or the single global talc) — no remote-free queue needed. This is why Step 1b
preceded Step 2.

**Missed-wake safety:** `ReplyFuture::poll` registers the waker THEN re-checks `done`
(no lost wake if `complete` races between the first check and the registration). Both run
loops halt only when neither WAKE_PENDING nor inbox-pending is set, under IF-disable
(the `sti;hlt` shadow) — an IPI arriving in the window re-checks instead of sleeping.

**Placeholder scan:** `NN` = next changelog number resolved at execution. The ICR const
names in Task 1 are to be VERIFIED against the existing `send_ipi_all_but_self` (flagged).
No TBD.

**Risk:** targeted-IPI destination encoding (xAPIC physical dest in ICR_HIGH bits 24-31)
is the most error-prone part — Task 4's round-trip test is the gate. If sum=10 never
prints, the IPI isn't reaching core 1 (or the AP isn't draining) — debug there before
declaring Step 2 done. This is concurrency-critical: do not mark done on a TIMEOUT.
