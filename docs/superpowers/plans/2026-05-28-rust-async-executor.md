# Async Executor Implementation Plan (Step 9)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `vfs::block_on` (noop_waker, single-future runtime from Step 7) with a real cooperative executor; prove multi-task interleaving and IRQ-driven wakes (timer + keyboard).

**Architecture:** `embassy-executor` 0.6 with `default-features=false` and no `arch-*` feature → we supply a custom `__pender` (no-op; idle is `hlt` inside embassy's run loop, woken naturally by IRQs). Hand-rolled `Delay(ticks)` future + global slot list scanned by the timer ISR. Keyboard ISR refactored to push into an async-aware queue (drops the synchronous `kprintln` path). `kmain` retains `vfs::block_on` for init-time work (Step 7 fixture stays), then hands off to `executor::run()` which never returns.

**Tech Stack:** Rust nightly-2026-05-26, `no_std` + `alloc`, target `x86_64-unknown-none`. New dep: `embassy-executor = "0.6"` with `nightly`, `task-arena-size-4096`. Existing: `spin`, `x86_64`.

**Spec:** `docs/superpowers/specs/2026-05-28-rust-async-executor-design.md`

**Branch:** `feature/async-executor` (already created from `main`)

**Build host:** All build/test commands run inside WSL Ubuntu:
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'
```

**TDD model for a kernel:** There is no `cargo test` for kernel code. The "test" is `make run-test`, which boots the kernel in QEMU headless with serial → stdout and `grep -qF`s for the `HELLO` sentinel in the Makefile. Each task's TDD cycle:
1. Change `HELLO` to the new expected sentinel (= the failing test).
2. Run `make run-test` → observe FAIL (sentinel not in log yet).
3. Implement the code that emits the sentinel.
4. Run `make build` → expect clean (pre-existing warnings only).
5. Run `make run-test` → observe PASS.
6. Commit.

**Changelog rule:** Every commit on this branch creates a new `CHANGELOG/NN-26-05-28-<slug>.md`. Pre-existing on this branch: `53` (spec), `54` (this plan). Implementer starts at `55`.

**Co-author trailer (mandatory in every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

**Git identity inside WSL (commits must show as `g.desolda`, not root):**
Use `git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit …` for every commit, OR `git config user.name "g.desolda" && git config user.email "g.desolda@gmail.com"` once at branch start.

---

## File Structure

**New files:**
- `kernel/src/executor/mod.rs` — Executor wrapper: `static EXECUTOR`, `pub fn run() -> !`, `__pender` no-op, the demo tasks (`tick_task`, `kbd_echo_task`).
- `kernel/src/executor/delay.rs` — `Delay(ticks)` future, global `SLOTS_LIST` of 8 entries, `pub fn timer_tick(now: u64)` consumed by timer ISR.
- `kernel/src/keyboard/queue.rs` — Async byte queue: `push_from_isr`, `read_char() -> ReadChar` future.

**Files moved / converted:**
- `kernel/src/keyboard.rs` → `kernel/src/keyboard/mod.rs` (file becomes a directory module so it can host `queue.rs` as a submodule).

**Files modified:**
- `kernel/Cargo.toml` — add `embassy-executor` dep.
- `kernel/src/main.rs` — add `mod executor;` declaration, replace the final `loop { x86_64::instructions::hlt(); }` block with `executor::run()`.
- `kernel/src/timer.rs` — `timer_handler` calls `crate::executor::delay::timer_tick(now)`.
- `kernel/src/keyboard/mod.rs` — ISR pushes into `queue::push_from_isr` instead of `kprintln!`.
- `Makefile` — update `HELLO` sentinel.

**Test contract evolution per task:**
| Task | HELLO before | HELLO after |
|------|---------------|-------------|
| 1    | `ruos: ticks=` | `ruos: executor up` |
| 2    | `ruos: executor up` | `ruos: async tick=2` |
| 3    | `ruos: async tick=2` | `ruos: async tick=2` (no change; keyboard echo verified manually) |

---

## Task 1: Executor scaffolding + bootstrap task

**Files:**
- Modify: `kernel/Cargo.toml`
- Create: `kernel/src/executor/mod.rs`
- Modify: `kernel/src/main.rs` (add `mod executor;`, replace final `loop { hlt }`)
- Modify: `Makefile` (`HELLO`)

**What we're building:** Minimum viable executor integration. Adds the dependency, creates the module, swaps `kmain`'s final idle loop for `executor::run()`. A single `bootstrap_task` prints `ruos: executor up` then `core::future::pending().await` (parks forever). Proves embassy compiles for `x86_64-unknown-none` with our custom `__pender`, that the executor links, that `kmain` can hand off, and that the kernel doesn't crash after the handoff (timer IRQ still fires inside embassy's idle loop, cursor still blinks).

**Test contract:** boot serial log contains `ruos: executor up`.

- [ ] **Step 1.1: Set the failing test sentinel**

Edit `Makefile`. Find the line that defines `HELLO`. It is currently:

```makefile
HELLO := ruos: ticks=
```

Replace with:

```makefile
HELLO := ruos: executor up
```

- [ ] **Step 1.2: Run the test to verify it fails**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 30 make run-test 2>&1 | tail -20'
```

Expected: `make: *** [Makefile:NN: run-test] Error 1` (grep didn't find the sentinel). The current kernel still emits `ruos: ticks=...` but not `ruos: executor up`.

- [ ] **Step 1.3: Add the embassy-executor dependency**

Edit `kernel/Cargo.toml`. In the `[dependencies]` section (currently ends with the `vte` line), append:

```toml
embassy-executor = { version = "0.6", default-features = false, features = ["nightly", "task-arena-size-4096"] }
```

The intent is `default-features = false` so we get *no* `arch-*` feature and supply `__pender` ourselves. `nightly` enables `async fn` in trait support inside embassy macros. `task-arena-size-4096` gives ~3-5 small tasks of headroom in the static slab.

- [ ] **Step 1.4: Create the executor module**

Create `kernel/src/executor/mod.rs` with:

```rust
//! Cooperative async executor for ruos.
//!
//! Built on `embassy-executor` with a custom `__pender` because
//! the `x86_64-unknown-none` target isn't covered by any built-in
//! `arch-*` feature. The kernel hands off to `run()` after init
//! completes; from there on, all forward progress is task-driven.
//!
//! Idle = `hlt` inside embassy's own run loop (it parks until a
//! `Pender` signal arrives or — equivalently for us — until any
//! IRQ returns control to the loop, at which point it re-polls
//! the run queue).

use embassy_executor::Executor;
use crate::kprintln;

// Embassy's `Executor::new` is `const`, so a plain `static mut` is the
// shortest path. Touched only by `run()`, which kmain calls exactly once.
static mut EXECUTOR: Executor = Executor::new();

/// Drive the kernel forever as a cooperative task system.
///
/// Never returns. After this point, the only way to make forward
/// progress is to spawn or wake a task.
pub fn run() -> ! {
    // SAFETY: called exactly once from kmain after init. No other
    // code references EXECUTOR.
    let exec: &'static mut Executor = unsafe { &mut *core::ptr::addr_of_mut!(EXECUTOR) };
    exec.run(|spawner| {
        spawner.spawn(bootstrap_task()).unwrap();
    })
}

/// Minimal task that proves the executor links and runs. Prints the
/// expected boot sentinel, then parks forever so the executor never
/// runs out of tasks. Later tasks (Task 2, Task 3) replace this with
/// real work.
#[embassy_executor::task]
async fn bootstrap_task() {
    kprintln!("ruos: executor up");
    core::future::pending::<()>().await;
}

/// Pender signal hook required by `embassy-executor` when no `arch-*`
/// feature is selected. Embassy calls this from inside a Waker's
/// `wake()` to nudge the executor's idle hook into rechecking the run
/// queue. For us the idle hook *is* the executor's main loop returning
/// from `hlt`, so nothing extra is needed here.
#[no_mangle]
extern "Rust" fn __pender(_context: *mut ()) {}
```

- [ ] **Step 1.5: Wire the module into the crate**

Edit `kernel/src/main.rs`. Find the existing `mod` declarations near the top (around line 15-20: `mod console;`, `mod vfs;`, etc.). Add anywhere in that block:

```rust
mod executor;
```

- [ ] **Step 1.6: Hand off kmain to the executor**

Edit `kernel/src/main.rs`. Find the final loop in `kmain`. It currently looks like (around the end of the function):

```rust
loop {
    let t = timer::ticks();
    kprintln!("ruos: ticks={}", t);
    for _ in 0..50_000_000u64 { core::hint::spin_loop(); }
}
```

(The exact body is one of: a `ticks` print loop, or just `loop { x86_64::instructions::hlt(); }`. Both forms exist in our history; replace whichever is current.)

Replace the entire loop with:

```rust
executor::run();
```

The `!` return type of `executor::run` satisfies `kmain`'s `-> !` requirement (no trailing `loop` needed).

- [ ] **Step 1.7: Build clean**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -30'
```

Expected: `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in NN.NNs`. Pre-existing warnings (12 today, list documented in CHANGELOG 50) may persist; no new warnings, no errors.

If embassy-executor 0.6 fails to compile against nightly-2026-05-26 with custom `__pender`, fall back to `embassy-executor = "0.5"` (same feature flags). The `__pender` symbol is stable from 0.4 onward; only minor API differences.

- [ ] **Step 1.8: Run the test to verify it passes**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 30 make run-test 2>&1 | tail -30'
```

Expected (last lines of the serial dump before timeout):
```
ruos: vfs smoke ok n=3 buf=[abc]
ruos: fb ok 1280x800 pitch=5120 bpp=32
ruos: fb test ok
ruos: fb attached
[31mERR[0m hello via ansi
ruos: ansi test ok
ruos: executor up
make: *** [Makefile:NN: run-test] Terminated
```

The `Terminated` line means the test passed (Makefile exits 0 on first sentinel match and the `timeout 30` then kills QEMU). Verify the `executor up` line is present.

- [ ] **Step 1.9: Create the changelog entry**

Create `CHANGELOG/55-26-05-28-async-executor-scaffold.md`:

```markdown
# 55 — Async executor scaffolding (Step 9 Task 1)

**Data:** 2026-05-28

## Cosa

- `embassy-executor` 0.6 aggiunto a `kernel/Cargo.toml`, no `arch-*`,
  feature `nightly` + `task-arena-size-4096`.
- Nuovo `kernel/src/executor/mod.rs`: `static EXECUTOR`, `pub fn run()`,
  `__pender` no-op, `bootstrap_task` che stampa `ruos: executor up` e
  parka su `core::future::pending`.
- `kmain` sostituisce il loop finale con `executor::run()`.
- `Makefile` HELLO → `ruos: executor up`.

## Perché

Primo dei 3 task dello Step 9. Mette in piedi l'executor e dimostra
che embassy compila e linka per `x86_64-unknown-none` col nostro
pender custom. Idle CPU resta `hlt` (embassy lo gestisce internamente)
e il timer IRQ continua a fire (cursor blink visibile).

## File toccati

- kernel/Cargo.toml
- kernel/src/executor/mod.rs (nuovo)
- kernel/src/main.rs
- Makefile
- CHANGELOG/55-26-05-28-async-executor-scaffold.md (nuovo)
```

- [ ] **Step 1.10: Commit**

```bash
git add kernel/Cargo.toml kernel/Cargo.lock kernel/src/executor/mod.rs kernel/src/main.rs Makefile CHANGELOG/55-26-05-28-async-executor-scaffold.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): scaffold async executor (embassy + custom Pender)

embassy-executor 0.6 with default-features=false, no arch-*, plus a
custom __pender (no-op — embassy's idle loop already returns from hlt
on every IRQ). bootstrap_task emits 'ruos: executor up' then parks
via core::future::pending so the executor never runs out of tasks.

kmain's final idle loop is replaced by executor::run(). vfs::block_on
remains intact for init-time async (Step 7 fixture); the executor
takes over for steady-state only.

Makefile HELLO advances to the new sentinel.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `Delay(ticks)` future + `tick_task`

**Files:**
- Create: `kernel/src/executor/delay.rs`
- Modify: `kernel/src/executor/mod.rs` (declare submodule; swap `bootstrap_task` for `tick_task`)
- Modify: `kernel/src/timer.rs` (handler calls `delay::timer_tick`)
- Modify: `Makefile` (`HELLO`)

**What we're building:** A minimal time-based future. `Delay::ticks(n)` registers a waker in a global 8-slot list; the timer ISR, on each fire, scans the list and wakes any due slot. `tick_task` loops awaiting `Delay::ticks(100)` (= 1 s @ 100 Hz) and prints `ruos: async tick={n}`. This proves (B) cooperative scheduling — `tick_task` interleaves with embassy's internal bookkeeping — and (C) IRQ-driven wake — the timer ISR is what produces forward progress.

**Test contract:** boot serial log contains `ruos: async tick=2` (= at least three async ticks have happened, ~3 s into boot).

- [ ] **Step 2.1: Set the failing test sentinel**

Edit `Makefile`. Change:

```makefile
HELLO := ruos: executor up
```

to:

```makefile
HELLO := ruos: async tick=2
```

- [ ] **Step 2.2: Run the test to verify it fails**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 30 make run-test 2>&1 | tail -20'
```

Expected: Error (grep can't find `ruos: async tick=2`; `bootstrap_task` from Task 1 only prints `executor up`).

- [ ] **Step 2.3: Create the Delay module**

Create `kernel/src/executor/delay.rs`:

```rust
//! Tick-based `Delay` future for ruos.
//!
//! Each `Delay` future, when polled, registers its waker into a global
//! 8-slot fixed list along with a target `TICKS` value. The timer ISR
//! scans the list on every fire and wakes any slot whose target has
//! been reached. The future's `Drop` impl clears the slot to handle
//! cancellation.
//!
//! The list is protected by `spin::Mutex`. Task-side accesses are
//! wrapped in `without_interrupts` to avoid a deadlock if the timer
//! IRQ fires while the lock is held. The ISR-side uses `try_lock` and
//! defers wakes to the next tick if the lock is contended (max 10 ms
//! latency at our 100 Hz tick rate — accepted trade-off).

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;

const SLOTS: usize = 8;

struct Slot {
    target: u64,
    waker: Waker,
}

// One global wake list; each `Delay` future occupies at most one slot
// at a time (idx is recorded in the future itself).
static SLOTS_LIST: Mutex<[Option<Slot>; SLOTS]> = Mutex::new([
    None, None, None, None, None, None, None, None,
]);

/// Future that resolves once `TICKS` has advanced by `n` from creation.
pub struct Delay {
    target: u64,
    slot: Option<usize>,
}

impl Delay {
    /// Construct a `Delay` that resolves after `n` timer ticks from
    /// the moment this is called (NOT the moment of first poll).
    pub fn ticks(n: u64) -> Self {
        let now = crate::timer::ticks();
        Delay { target: now.saturating_add(n), slot: None }
    }

    fn free_slot(&mut self) {
        if let Some(idx) = self.slot.take() {
            without_interrupts(|| {
                let mut list = SLOTS_LIST.lock();
                list[idx] = None;
            });
        }
    }
}

impl Future for Delay {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        // Delay has only Unpin fields (u64, Option<usize>), so it auto-
        // implements Unpin and `get_mut` is safe.
        let me = self.get_mut();

        if crate::timer::ticks() >= me.target {
            me.free_slot();
            return Poll::Ready(());
        }

        without_interrupts(|| {
            let mut list = SLOTS_LIST.lock();
            // Already registered: update the waker (poll may be called
            // by a different waker than the previous one — rare in our
            // single-executor world, but cheap to be correct).
            if let Some(idx) = me.slot {
                list[idx] = Some(Slot {
                    target: me.target,
                    waker: cx.waker().clone(),
                });
                return Poll::Pending;
            }
            // Find a free slot.
            for (i, s) in list.iter_mut().enumerate() {
                if s.is_none() {
                    *s = Some(Slot {
                        target: me.target,
                        waker: cx.waker().clone(),
                    });
                    me.slot = Some(i);
                    return Poll::Pending;
                }
            }
            panic!("ruos: delay slots exhausted ({} in use)", SLOTS);
        })
    }
}

impl Drop for Delay {
    fn drop(&mut self) {
        self.free_slot();
    }
}

/// Called from the timer ISR (`timer::timer_handler`) on every fire.
///
/// Walks the slot list and wakes every slot whose `target` has been
/// reached. Uses `try_lock` so a contended list never deadlocks the
/// ISR; missed slots are picked up on the next tick (max 10 ms delay).
pub fn timer_tick(now: u64) {
    if let Some(mut list) = SLOTS_LIST.try_lock() {
        for s in list.iter_mut() {
            // Match on a borrow first, then mutate, so we can pull the
            // waker out without leaving the slot in a half-empty state.
            let due = matches!(s, Some(entry) if entry.target <= now);
            if due {
                if let Some(entry) = s.take() {
                    entry.waker.wake();
                }
            }
        }
    }
}
```

- [ ] **Step 2.4: Declare the submodule in `executor/mod.rs`**

Edit `kernel/src/executor/mod.rs`. Just after the file's module-level doc comment (the `//!` block at the top), add:

```rust
pub mod delay;
```

- [ ] **Step 2.5: Wire the timer ISR to drive the delay list**

Edit `kernel/src/timer.rs`. The current `timer_handler` is:

```rust
pub extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    crate::console::fb::tick_cursor();
    lapic::eoi();
}
```

Replace with:

```rust
pub extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    // fetch_add returns the *previous* value, so add 1 to get "now".
    let now = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    crate::console::fb::tick_cursor();
    crate::executor::delay::timer_tick(now);
    lapic::eoi();
}
```

- [ ] **Step 2.6: Replace `bootstrap_task` with `tick_task`**

Edit `kernel/src/executor/mod.rs`. Find the existing `bootstrap_task`:

```rust
#[embassy_executor::task]
async fn bootstrap_task() {
    kprintln!("ruos: executor up");
    core::future::pending::<()>().await;
}
```

Replace with:

```rust
#[embassy_executor::task]
async fn tick_task() {
    kprintln!("ruos: executor up");
    let mut n: u32 = 0;
    loop {
        delay::Delay::ticks(100).await; // 1s @ 100 Hz
        kprintln!("ruos: async tick={}", n);
        n = n.wrapping_add(1);
    }
}
```

Update the `spawner.spawn(...)` call inside `run()` accordingly. The body of `run` becomes:

```rust
pub fn run() -> ! {
    let exec: &'static mut Executor = unsafe { &mut *core::ptr::addr_of_mut!(EXECUTOR) };
    exec.run(|spawner| {
        spawner.spawn(tick_task()).unwrap();
    })
}
```

(The `executor up` line moves into `tick_task` so the same sentinel still appears before the first delay — useful to debug if the delay infrastructure misbehaves: if you see `executor up` but no `async tick=`, the executor is alive but `Delay` is broken.)

- [ ] **Step 2.7: Build clean**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -30'
```

Expected: `Finished` line, no new errors. New warning count == Task 1 baseline.

- [ ] **Step 2.8: Run the test to verify it passes**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 30 make run-test 2>&1 | tail -30'
```

Expected serial tail:
```
ruos: ansi test ok
ruos: executor up
ruos: async tick=0
ruos: async tick=1
ruos: async tick=2
make: *** [Makefile:NN: run-test] Terminated
```

The three async tick lines must be present in order. If the kernel emits `executor up` but no `tick=` line, the timer ISR isn't waking the delay list — re-check Step 2.5. If only `tick=0` is emitted then the executor hangs after first wake — re-check Step 2.6 (the loop must register a *new* Delay each iteration).

- [ ] **Step 2.9: Create the changelog entry**

Create `CHANGELOG/56-26-05-28-async-delay-tick-task.md`:

```markdown
# 56 — Delay future + tick_task (Step 9 Task 2)

**Data:** 2026-05-28

## Cosa

- `kernel/src/executor/delay.rs`: `Delay(target_ticks)` future + list
  globale di 8 slot (`Mutex<[Option<Slot>; 8]>`). `timer_tick(now)`
  chiamata dal timer ISR via `try_lock` per evitare deadlock.
  Senders wrappano in `without_interrupts`.
- `timer::timer_handler` ora invoca `delay::timer_tick(now)` dopo
  `tick_cursor` e prima di `eoi`.
- `executor::tick_task` rimpiazza `bootstrap_task`: loop di
  `Delay::ticks(100).await` + `kprintln!("ruos: async tick={n}")`.
- `Makefile` HELLO → `ruos: async tick=2`.

## Perché

Secondo dei 3 task dello Step 9. Materializza i due requisiti chiave
del milestone: (B) un task asincrono che ritorna allo scheduler ad
ogni iterazione, (C) wake da IRQ (timer) verso il future.

## File toccati

- kernel/src/executor/delay.rs (nuovo)
- kernel/src/executor/mod.rs
- kernel/src/timer.rs
- Makefile
- CHANGELOG/56-26-05-28-async-delay-tick-task.md (nuovo)
```

- [ ] **Step 2.10: Commit**

```bash
git add kernel/src/executor/delay.rs kernel/src/executor/mod.rs kernel/src/timer.rs Makefile CHANGELOG/56-26-05-28-async-delay-tick-task.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): Delay future + timer-IRQ wake + tick_task demo

Hand-rolled Delay(target_ticks) future with a fixed 8-slot global list.
poll() registers (target, Waker) under without_interrupts; Drop clears
the slot for cancellation. timer_handler now calls delay::timer_tick
via try_lock and wakes any due slot.

tick_task awaits Delay::ticks(100) (= 1s @ 100 Hz) in a loop, prints
'ruos: async tick={n}'. Boot smoke advances to grep async tick=2,
proving multi-iteration scheduling AND timer-IRQ-driven wake.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Keyboard async queue + `kbd_echo_task`

**Files:**
- Move: `kernel/src/keyboard.rs` → `kernel/src/keyboard/mod.rs`
- Create: `kernel/src/keyboard/queue.rs`
- Modify: `kernel/src/keyboard/mod.rs` (ISR pushes into queue instead of `kprintln`)
- Modify: `kernel/src/executor/mod.rs` (add `kbd_echo_task`, spawn it)

**What we're building:** Second IRQ-wake source. The PS/2 keyboard ISR no longer prints directly; it pushes a decoded byte into a 64-byte ring buffered async queue, then wakes any pending `read_char()` future. A new `kbd_echo_task` spawns alongside `tick_task` and, in a loop, awaits `keyboard::queue::read_char()` and prints `ruos: kbd echo='{c}'`. Proves wake from a non-timer ISR; sets up Step 11 (shell) to consume keyboard input as a stream.

**Test contract (automated):** unchanged from Task 2 (`ruos: async tick=2`). Keyboard echo is not driven by `make run-test` (no stdin to QEMU in the test harness), so a regression in `tick_task` would still surface — the explicit sentinel for keyboard echo is verified manually below.

**Test contract (manual):** `make run` launches an interactive QEMU. Pressing `a` produces a serial line `ruos: kbd echo='a'` within ~10 ms.

- [ ] **Step 3.1: Convert `keyboard.rs` into a directory module**

The current single-file module needs to grow a submodule. Move the file:

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && git mv kernel/src/keyboard.rs kernel/src/keyboard/mod.rs'
```

- [ ] **Step 3.2: Build clean (smoke after move)**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -20'
```

Expected: no change in behavior, no new warnings, no errors. The compiler picks up the directory module form transparently.

- [ ] **Step 3.3: Create the async keyboard queue**

Create `kernel/src/keyboard/queue.rs`:

```rust
//! Async-aware keyboard byte queue.
//!
//! The keyboard ISR (`keyboard::handler`) decodes a scancode into an
//! ASCII byte and pushes it via `push_from_isr`. A consumer task awaits
//! `read_char()`, which suspends on a stored `Waker` until the next
//! byte arrives.
//!
//! Concurrency:
//! - ISR side: holds the `STATE` lock briefly to push + take the
//!   waker; calls `wake()` *outside* the lock to avoid re-entering
//!   the executor while holding the queue.
//! - Task side: wraps `STATE.lock()` in `without_interrupts` so the
//!   keyboard IRQ can't preempt it mid-critical-section and deadlock.
//!
//! Overflow policy: drop the byte and bump `DROPPED`. A 64-byte buffer
//! at 100 keystrokes/s gives ~640 ms of slack — plenty for cooperative
//! scheduling.

use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicU64, Ordering};
use core::task::{Context, Poll, Waker};
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;

const BUF_LEN: usize = 64;

struct State {
    buf: [u8; BUF_LEN],
    head: usize, // next-to-read
    tail: usize, // next-to-write
    waker: Option<Waker>,
}

impl State {
    const fn new() -> Self {
        Self {
            buf: [0; BUF_LEN],
            head: 0,
            tail: 0,
            waker: None,
        }
    }
    fn is_empty(&self) -> bool { self.head == self.tail }
    fn is_full(&self) -> bool { (self.tail + 1) % BUF_LEN == self.head }
}

static STATE: Mutex<State> = Mutex::new(State::new());
pub static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Called by the keyboard ISR for each decoded byte. Non-blocking;
/// drops the byte if the queue is full (incrementing `DROPPED`).
pub fn push_from_isr(b: u8) {
    let waker = {
        let mut s = STATE.lock();
        if s.is_full() {
            DROPPED.fetch_add(1, Ordering::Relaxed);
            None
        } else {
            s.buf[s.tail] = b;
            s.tail = (s.tail + 1) % BUF_LEN;
            s.waker.take()
        }
    };
    // Wake outside the lock — wake() may eventually re-enter the
    // executor, and we don't want to nest the locks.
    if let Some(w) = waker {
        w.wake();
    }
}

/// Async future that resolves to the next available byte.
pub fn read_char() -> ReadChar { ReadChar }

pub struct ReadChar;

impl Future for ReadChar {
    type Output = u8;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u8> {
        without_interrupts(|| {
            let mut s = STATE.lock();
            if s.is_empty() {
                s.waker = Some(cx.waker().clone());
                Poll::Pending
            } else {
                let b = s.buf[s.head];
                s.head = (s.head + 1) % BUF_LEN;
                Poll::Ready(b)
            }
        })
    }
}
```

- [ ] **Step 3.4: Wire the queue submodule and refactor the ISR**

Edit `kernel/src/keyboard/mod.rs`. Near the top (after the `//!` doc comment, if any), add:

```rust
pub mod queue;
```

Find the keyboard ISR (the `extern "x86-interrupt" fn` that handles IRQ 1). It currently decodes the scancode and calls `kprintln!` directly. The exact body looks roughly like (locate the `pc-keyboard` Keyboard usage):

```rust
pub extern "x86-interrupt" fn keyboard_handler(_frame: InterruptStackFrame) {
    // ... port read, scancode decode ...
    if let Some(DecodedKey::Unicode(c)) = decoded {
        kprintln!("ruos: kbd '{}'", c);
    }
    crate::apic::lapic::eoi();
}
```

(Or similar — the structure has the IRQ → port read → decode → kprintln pipeline.)

Replace the `kprintln!` call(s) with `queue::push_from_isr`. The new tail of the handler:

```rust
if let Some(DecodedKey::Unicode(c)) = decoded {
    // Push the byte into the async queue; the consumer task is in
    // charge of any logging. Non-ASCII chars are clamped to ASCII for
    // the queue (the consumer is going to ASCII-echo in this step;
    // Step 11 will expand once we have a real input layer).
    let b = if (c as u32) < 0x80 { c as u8 } else { b'?' };
    queue::push_from_isr(b);
}
crate::apic::lapic::eoi();
```

If the existing handler also has a `DecodedKey::RawKey(...)` arm or a fallback that already prints, drop those prints too — leave only the queue push for ASCII bytes; everything else is silently discarded (Step 11 will revisit). The keyboard ISR must produce *no* `kprintln!` from this task onward.

- [ ] **Step 3.5: Add and spawn `kbd_echo_task`**

Edit `kernel/src/executor/mod.rs`. Add this task definition near `tick_task`:

```rust
#[embassy_executor::task]
async fn kbd_echo_task() {
    loop {
        let b = crate::keyboard::queue::read_char().await;
        kprintln!("ruos: kbd echo={:?}", b as char);
    }
}
```

Update `run()`'s spawn closure to spawn both:

```rust
pub fn run() -> ! {
    let exec: &'static mut Executor = unsafe { &mut *core::ptr::addr_of_mut!(EXECUTOR) };
    exec.run(|spawner| {
        spawner.spawn(tick_task()).unwrap();
        spawner.spawn(kbd_echo_task()).unwrap();
    })
}
```

- [ ] **Step 3.6: Build clean**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -30'
```

Expected: `Finished` line, no new warnings, no errors.

- [ ] **Step 3.7: Run the test to verify automated smoke still passes**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 30 make run-test 2>&1 | tail -20'
```

Expected: the same `ruos: async tick=2` sentinel matches and the test terminates clean. Adding `kbd_echo_task` must not regress `tick_task`'s scheduling.

- [ ] **Step 3.8: Manual smoke (keyboard echo)**

This step is interactive — execute it directly in a terminal that can drive QEMU, not via subagent dispatch.

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run'
```

In QEMU's window, press `a`. Within a frame or two, the serial console should display:

```
ruos: kbd echo='a'
```

Press `b`, `c`, `d`. Expect:

```
ruos: kbd echo='b'
ruos: kbd echo='c'
ruos: kbd echo='d'
```

Quit QEMU (Ctrl-A, X in the serial window, or close the QEMU window).

If `kbd echo` does NOT appear:
- If `tick=N` is still emitted: the executor is alive; the keyboard path is broken. Double-check Step 3.4 (push_from_isr is called) and Step 3.3 (Waker is stored and woken).
- If nothing is emitted at all after `ansi test ok`: the executor is wedged. Roll back Task 3 by `git reset --hard` to Task 2's commit and investigate.

If the subagent executing this plan cannot drive an interactive QEMU, it must STOP and report `NEEDS_HUMAN_VERIFICATION` for this step. Do not skip and proceed to commit without manual verification — the kbd echo path is the only validation of Step 9 deliverable (C, second IRQ source).

- [ ] **Step 3.9: Create the changelog entry**

Create `CHANGELOG/57-26-05-28-async-keyboard-queue.md`:

```markdown
# 57 — Keyboard async queue + kbd_echo_task (Step 9 Task 3)

**Data:** 2026-05-28

## Cosa

- `kernel/src/keyboard.rs` → `kernel/src/keyboard/mod.rs` (modulo
  diretta·rio, per ospitare `queue.rs`).
- `kernel/src/keyboard/queue.rs`: ring buffer 64 byte protetto da
  `spin::Mutex`, `push_from_isr` + `read_char()` future. Senders
  wrappano in `without_interrupts`; ISR wake fuori dal lock.
- ISR `keyboard_handler` non chiama più `kprintln!`: pusha solo
  nel queue. Non-ASCII byte mappati a `'?'` (Step 11 espanderà).
- `executor::kbd_echo_task` aggiunto, spawnato accanto a `tick_task`
  in `executor::run`.
- Test automatico invariato (`ruos: async tick=2` ancora HELLO);
  verifica keyboard manuale via `make run`.

## Perché

Terzo e ultimo task dello Step 9. Materializza il requisito (C) con
una *seconda* sorgente di IRQ-wake (keyboard ≠ timer), prova che il
pattern Waker funziona generalmente. Sblocca Step 11 (shell consumer
del queue) e Step 12 (PTY line discipline a monte).

## File toccati

- kernel/src/keyboard.rs → kernel/src/keyboard/mod.rs (rinominato + mod)
- kernel/src/keyboard/queue.rs (nuovo)
- kernel/src/executor/mod.rs (kbd_echo_task + spawn)
- CHANGELOG/57-26-05-28-async-keyboard-queue.md (nuovo)
```

- [ ] **Step 3.10: Commit**

```bash
git add kernel/src/keyboard/ kernel/src/executor/mod.rs CHANGELOG/57-26-05-28-async-keyboard-queue.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): keyboard async queue + kbd_echo_task

PS/2 keyboard ISR no longer prints directly; it pushes the decoded
byte into a 64-byte spin::Mutex-protected ring buffer and wakes the
pending read_char() Waker. Non-ASCII keys clamp to '?' for now;
Step 11 will revisit when the shell becomes the real consumer.

kbd_echo_task awaits read_char in a loop and prints
'ruos: kbd echo='X''. Verified manually in QEMU; the make run-test
sentinel is unchanged (async tick=2) so this commit also exercises
the no-regression contract on tick_task.

Step 9 milestone complete: cooperative executor + Delay future +
two IRQ wake sources (timer @ 100 Hz, keyboard @ IRQ 1).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-review checks (run by the controller before merging)

**Spec coverage:**
| Spec requirement | Implemented by |
|------------------|----------------|
| embassy-executor 0.6, no `arch-*`, custom `__pender` | Task 1 (Cargo.toml + executor/mod.rs `__pender`) |
| kmain shape α (block_on init + embassy steady-state) | Task 1 (kmain hand-off) |
| `Delay(target_ticks)` future | Task 2 (executor/delay.rs) |
| `DelayList` 8 slot, `Mutex` + `without_interrupts` discipline | Task 2 |
| `timer_handler` calls `delay::timer_tick(now)` | Task 2 |
| Keyboard ISR pushes into async queue, no more `kprintln` from ISR | Task 3 |
| `KbdQueue` 64-byte ring + DROPPED counter + Waker | Task 3 |
| `tick_task` smoke `ruos: async tick={n}` | Task 2 |
| `kbd_echo_task` smoke `ruos: kbd echo='X'` | Task 3 |
| Makefile HELLO updated | Tasks 1 & 2 |

**Out of scope verified out of scope:** no preemption, no SMP, no embassy-time, no dynamic spawn — none appear in the tasks above.

**Type/name consistency:** `Delay::ticks(n)`, `delay::timer_tick(now)`, `queue::push_from_isr(b)`, `queue::read_char()` — names match between definition (Tasks 2/3) and call sites (timer.rs, keyboard/mod.rs, executor/mod.rs).

**Stuck-task escape hatch:** if Task 3.8 (manual smoke) can't be verified by the dispatched implementer, the task escalates to `NEEDS_HUMAN_VERIFICATION` and the controller (the user, or a parent agent with terminal access) takes over the last verification.

---

## After all tasks complete

1. Run `make build` clean.
2. Run `make run-test` clean (assertion `ruos: async tick=2`).
3. Dispatch a final whole-implementation reviewer (see `superpowers:code-reviewer` agent) — review the diff `main..feature/async-executor`, focus on ISR safety, lock disciplines, and embassy integration correctness.
4. Address any blockers; non-blockers go into `docs/followups/step-9.md` (mirror the Step 8 pattern).
5. Merge `feature/async-executor` → `main` no-ff, push to `origin/main`, delete the local branch.
