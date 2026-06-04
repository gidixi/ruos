# Mouse PS/2 Driver Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a PS/2 mouse driver (IRQ12) that decodes 3-byte packets into movement/button events and exposes a drainable event queue, as the input foundation for the egui desktop.

**Architecture:** A new `kernel/src/mouse/` module mirrors the existing
`kernel/src/keyboard/mod.rs` driver: a pure `decode_packet` function (unit-checkable),
an IRQ-safe event ring buffer, an `extern "x86-interrupt"` ISR that assembles
packets and pushes events, and an `init()` that runs the PS/2 controller enable
sequence and wires IRQ12 through the IOAPIC. A new IDT vector `VEC_MOUSE = 0x22`
registers the handler. The driver is wired in boot phase 3 (`interrupts`), right
after the keyboard.

**Tech Stack:** Rust `no_std`, `x86_64` crate (`Port`, `InterruptStackFrame`),
existing `crate::apic`, `crate::idt`, `crate::sync::IrqMutex`, `alloc::collections::VecDeque`.

**Testing model (ruos-specific):** the kernel is `no_std` and has no host
`cargo test`. The project's test analog is a `#[cfg(feature = "boot-checks")]`
self-test function that runs at boot and logs a pass/fail line to serial (see
`crate::console::fb::self_test` + `make test-boot`, Makefile target that greps
the boot log). Each "failing test" step here means: add/extend a boot-check
self-test, build with `boot-checks`, run `make test-boot`, and observe the
expected fail/pass line.

**This plan is #1 of a series** (see `docs/superpowers/specs/2026-06-04-egui-desktop-wasmtime-aot-design.md` §13):
1. **Mouse PS/2 driver** ← this plan
2. Exec-memory W^X in paging
3. Wasmtime no_std spike (decision gate)
4. `ruos_gfx` ABI + gfx service + console suspend/restore
5. `gui` app (egui backend + tiny-skia)
6. `ruos_proc` ABI + terminal emulator
7. Runtime router + build step + FPS benchmark

---

## File Structure

- Create: `kernel/src/mouse/mod.rs` — driver: types, `decode_packet`, event queue, ISR, `init`, `self_test`.
- Modify: `kernel/src/idt.rs` — add `VEC_MOUSE` constant + register handler.
- Modify: `kernel/src/boot/phases/interrupts.rs` — call `mouse::init` + boot-check self-test.
- Modify: `kernel/src/main.rs` (or crate root with the other `pub mod`s) — declare `pub mod mouse;`.

**Build/run commands** (per CLAUDE.md the build host is WSL). Run from the repo
root through WSL, e.g.:
```bash
wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && <cmd>'
```
Replace `<repo-on-wsl>` with this repo's WSL path (e.g. `/mnt/w/Work/GitHub/ruos`).
Where a step says `make <target>`, wrap it in that form.

---

## Task 1: Module skeleton + pure packet decoder

**Files:**
- Create: `kernel/src/mouse/mod.rs`
- Modify: `kernel/src/main.rs` (module declaration list)

- [ ] **Step 1: Declare the module**

In `kernel/src/main.rs`, find the block of `pub mod` / `mod` declarations
(where `keyboard` is declared) and add alongside it:

```rust
pub mod mouse;
```

Run to find the exact line:
`grep -n "mod keyboard" kernel/src/main.rs`
Add `pub mod mouse;` immediately after that line.

- [ ] **Step 2: Write the decoder + its boot-check self-test (failing test first)**

Create `kernel/src/mouse/mod.rs` with ONLY the types, decoder, and self-test for now:

```rust
//! PS/2 mouse driver (IRQ12). Standard 3-byte packet protocol.
//!
//! Mirrors `crate::keyboard`: a pure `decode_packet` turns a raw 3-byte packet
//! into a `MouseEvent`; the ISR assembles packets and pushes events into an
//! IRQ-safe ring drained by higher layers (later: the GUI gfx service).

/// One decoded mouse report. Movement is relative; Y is already flipped so
/// positive `dy` means "cursor moves down" (PS/2 reports up as positive).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct MouseEvent {
    pub dx: i16,
    pub dy: i16,
    pub left: bool,
    pub right: bool,
    pub middle: bool,
}

/// Decode a raw 3-byte PS/2 mouse packet.
///
/// byte0: bit0 L, bit1 R, bit2 M, bit3 always-1, bit4 X-sign, bit5 Y-sign,
///        bit6 X-overflow, bit7 Y-overflow.
/// byte1: X movement (9-bit two's complement with byte0 sign bit).
/// byte2: Y movement (likewise). Y is negated so down is positive.
pub fn decode_packet(b: [u8; 3]) -> MouseEvent {
    let flags = b[0];
    let left = flags & 0x01 != 0;
    let right = flags & 0x02 != 0;
    let middle = flags & 0x04 != 0;

    // Sign-extend the 8-bit movement using the sign bits in byte0.
    let dx = if flags & 0x10 != 0 {
        (b[1] as i16) - 0x100
    } else {
        b[1] as i16
    };
    let dy_raw = if flags & 0x20 != 0 {
        (b[2] as i16) - 0x100
    } else {
        b[2] as i16
    };

    MouseEvent { dx, dy: -dy_raw, left, right, middle }
}

/// Boot-check self-test: deterministic packets exercise sign-extension,
/// Y-flip, and button bits. Returns true on success.
#[cfg(feature = "boot-checks")]
pub fn self_test() -> bool {
    // No movement, no buttons.
    if decode_packet([0x08, 0x00, 0x00]) != (MouseEvent { dx: 0, dy: 0, left: false, right: false, middle: false }) {
        return false;
    }
    // Left button, +5 right, +3 up → dy flips to -3 (up).
    if decode_packet([0x09, 0x05, 0x03]) != (MouseEvent { dx: 5, dy: -3, left: true, right: false, middle: false }) {
        return false;
    }
    // X-sign + Y-sign set: byte1=0xFE → -2, byte2=0xFF → -1, dy flips to +1.
    if decode_packet([0x38, 0xFE, 0xFF]) != (MouseEvent { dx: -2, dy: 1, left: false, right: false, middle: true }) {
        return false;
    }
    true
}
```

- [ ] **Step 3: Wire the self-test into boot and run it to verify it FAILS (not yet called)**

In `kernel/src/boot/phases/interrupts.rs`, after the keyboard wiring block
(`crate::keyboard::init(...)` + its `binfo!`, around line 42), add:

```rust
    #[cfg(feature = "boot-checks")]
    {
        let ok = crate::mouse::self_test();
        crate::binfo!("mouse", "decode self-test {}", if ok { "ok" } else { "FAIL" });
    }
```

Run:
`wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && make test-boot'`
Expected at this point: **FAIL** — to confirm the harness sees this line, first
introduce a deliberate wrong expectation (temporarily change the first assertion
in `self_test` to `dx: 1`), rebuild, and confirm the log prints
`mouse: decode self-test FAIL`. This proves the test is real.

- [ ] **Step 4: Revert the deliberate break; run to verify it PASSES**

Restore the correct `dx: 0` expectation. Run:
`wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && make test-boot'`
Expected: boot log contains `mouse: decode self-test ok` AND the existing
`$(HELLO)` line (so the rest of boot still works).

- [ ] **Step 5: Commit**

```bash
git add kernel/src/mouse/mod.rs kernel/src/main.rs kernel/src/boot/phases/interrupts.rs
git commit -m "feat(mouse): pure PS/2 packet decoder + boot-check self-test"
```

---

## Task 2: IRQ-safe event queue

**Files:**
- Modify: `kernel/src/mouse/mod.rs`

- [ ] **Step 1: Add the event ring + public drain API, plus a self-test for it**

At the top of `kernel/src/mouse/mod.rs`, add imports and the queue. Append the
push/pop functions after `decode_packet`:

```rust
use alloc::collections::VecDeque;
use crate::sync::IrqMutex;

/// Bounded event queue. Oldest events are dropped when full so a fast-moving
/// mouse never blocks the ISR or grows memory without bound.
const QUEUE_CAP: usize = 256;
static QUEUE: IrqMutex<VecDeque<MouseEvent>> = IrqMutex::new(VecDeque::new());

/// Push an event from the ISR. Drops the oldest if at capacity.
fn push_event(ev: MouseEvent) {
    let mut q = QUEUE.lock();
    if q.len() >= QUEUE_CAP {
        q.pop_front();
    }
    q.push_back(ev);
}

/// Drain one event, if any. Called by higher layers (GUI gfx service).
pub fn pop_event() -> Option<MouseEvent> {
    QUEUE.lock().pop_front()
}

/// Number of queued events (test/diagnostic helper).
pub fn queued() -> usize {
    QUEUE.lock().len()
}
```

Verify the `IrqMutex::new` signature is `const fn` and `lock()` returns a guard:
`grep -n "impl.*IrqMutex\|pub const fn new\|pub fn lock" kernel/src/sync/*.rs`
If `IrqMutex::new` is NOT const, replace the static with
`static QUEUE: IrqMutex<VecDeque<MouseEvent>> = IrqMutex::new(VecDeque::new());`
→ use `spin::Once` lazy init instead: declare
`static QUEUE: spin::Once<IrqMutex<VecDeque<MouseEvent>>> = spin::Once::new();`
and a `fn queue() -> &'static IrqMutex<...> { QUEUE.call_once(|| IrqMutex::new(VecDeque::new())) }`,
routing `push_event`/`pop_event`/`queued` through `queue()`.

Extend `self_test` (inside the `#[cfg(feature = "boot-checks")]` fn, before
`true`) to exercise the queue:

```rust
    // Queue round-trips and respects FIFO order.
    push_event(MouseEvent { dx: 1, dy: 2, left: true, right: false, middle: false });
    push_event(MouseEvent { dx: 3, dy: 4, left: false, right: true, middle: false });
    let a = pop_event();
    let b = pop_event();
    let c = pop_event();
    if a != Some(MouseEvent { dx: 1, dy: 2, left: true, right: false, middle: false }) { return false; }
    if b != Some(MouseEvent { dx: 3, dy: 4, left: false, right: true, middle: false }) { return false; }
    if c != None { return false; }
```

- [ ] **Step 2: Run to verify it fails (compile-or-assert), then passes**

Run:
`wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && make test-boot'`
Expected: builds clean and log still shows `mouse: decode self-test ok`. If the
`IrqMutex::new` const issue triggers a compile error, apply the `spin::Once`
fallback above and re-run.

- [ ] **Step 3: Commit**

```bash
git add kernel/src/mouse/mod.rs
git commit -m "feat(mouse): IRQ-safe bounded event queue with drain API"
```

---

## Task 3: IDT vector for IRQ12

**Files:**
- Modify: `kernel/src/idt.rs`

- [ ] **Step 1: Add the vector constant**

In `kernel/src/idt.rs`, after `pub const VEC_KEYBOARD: u8 = 0x21;` (line 11), add:

```rust
pub const VEC_MOUSE: u8 = 0x22;
```

- [ ] **Step 2: Register the handler in the IDT**

In `kernel/src/idt.rs`, in the `init()` builder block, after
`idt[VEC_KEYBOARD].set_handler_fn(crate::keyboard::keyboard_handler);` (line 35), add:

```rust
        idt[VEC_MOUSE].set_handler_fn(crate::mouse::mouse_handler);
```

This references `crate::mouse::mouse_handler`, defined in Task 4. The build will
fail to link until Task 4 is done — that is expected and is the "failing test"
for this task.

- [ ] **Step 3: Run to verify it fails (unresolved handler)**

Run:
`wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && make iso CARGO_FEATURES=boot-checks'`
Expected: **compile error** `cannot find function mouse_handler in module crate::mouse`.
This confirms the IDT slot is wired to the upcoming ISR.

- [ ] **Step 4: Commit (with Task 4 — do not commit a non-building tree alone)**

Defer the commit; commit together at the end of Task 4.

---

## Task 4: The ISR (packet assembler)

**Files:**
- Modify: `kernel/src/mouse/mod.rs`

- [ ] **Step 1: Add the ISR + packet state machine**

In `kernel/src/mouse/mod.rs`, add imports at the top:

```rust
use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use x86_64::instructions::port::Port;
use x86_64::structures::idt::InterruptStackFrame;
use crate::apic;
```

Then add the packet-assembly state and the handler:

```rust
/// Index of the next byte within the current 3-byte packet (0,1,2).
static PKT_IDX: AtomicU8 = AtomicU8::new(0);
/// The two already-received bytes of the in-progress packet, packed as
/// (byte0 << 8) | byte1. byte2 is handled inline when it arrives.
static PKT_BUF: AtomicU32 = AtomicU32::new(0);

/// IRQ12 handler. Reads one byte from the PS/2 data port, assembles a 3-byte
/// packet, and on completion decodes + enqueues a `MouseEvent`.
pub extern "x86-interrupt" fn mouse_handler(_frame: InterruptStackFrame) {
    let mut data: Port<u8> = Port::new(0x60);
    // SAFETY: 0x60 is the PS/2 controller data port.
    let byte = unsafe { data.read() };

    let idx = PKT_IDX.load(Ordering::SeqCst);
    match idx {
        0 => {
            // Sync guard: byte0 must have bit3 set. If not, drop and resync.
            if byte & 0x08 == 0 {
                apic::lapic::eoi();
                return;
            }
            PKT_BUF.store((byte as u32) << 8, Ordering::SeqCst);
            PKT_IDX.store(1, Ordering::SeqCst);
        }
        1 => {
            let prev = PKT_BUF.load(Ordering::SeqCst);
            PKT_BUF.store(prev | (byte as u32), Ordering::SeqCst);
            PKT_IDX.store(2, Ordering::SeqCst);
        }
        _ => {
            let packed = PKT_BUF.load(Ordering::SeqCst);
            let b0 = (packed >> 8) as u8;
            let b1 = packed as u8;
            let ev = decode_packet([b0, b1, byte]);
            push_event(ev);
            PKT_IDX.store(0, Ordering::SeqCst);
        }
    }

    apic::lapic::eoi();
}
```

- [ ] **Step 2: Run to verify it builds (Task 3's link error is resolved)**

Run:
`wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && make test-boot'`
Expected: builds clean; boot log still shows `mouse: decode self-test ok` and
`$(HELLO)`. (No real movement asserted yet — see Task 6.)

- [ ] **Step 3: Commit (Tasks 3 + 4 together)**

```bash
git add kernel/src/idt.rs kernel/src/mouse/mod.rs
git commit -m "feat(mouse): IRQ12 ISR with 3-byte packet assembler + IDT vector 0x22"
```

---

## Task 5: PS/2 controller init + IOAPIC wiring

**Files:**
- Modify: `kernel/src/mouse/mod.rs`
- Modify: `kernel/src/boot/phases/interrupts.rs`

- [ ] **Step 1: Add the controller enable sequence + `init`**

In `kernel/src/mouse/mod.rs`, add the controller helpers and `init`. The PS/2
status port is `0x64` (command/status); data is `0x60`.

```rust
use crate::acpi_init::IrqOverride;

const PS2_DATA: u16 = 0x60;
const PS2_CMD: u16 = 0x64;

/// Spin until the controller's input buffer is empty (safe to write).
fn wait_write() {
    let mut status: Port<u8> = Port::new(PS2_CMD);
    for _ in 0..100_000 {
        // SAFETY: reading the PS/2 status port has no side effects.
        if unsafe { status.read() } & 0x02 == 0 {
            return;
        }
    }
}

/// Spin until the controller's output buffer is full (data available to read).
fn wait_read() {
    let mut status: Port<u8> = Port::new(PS2_CMD);
    for _ in 0..100_000 {
        // SAFETY: reading the PS/2 status port has no side effects.
        if unsafe { status.read() } & 0x01 != 0 {
            return;
        }
    }
}

/// Write a command byte to the PS/2 controller (port 0x64).
fn cmd(b: u8) {
    let mut p: Port<u8> = Port::new(PS2_CMD);
    wait_write();
    // SAFETY: 0x64 is the PS/2 command port.
    unsafe { p.write(b) };
}

/// Write a data byte to the PS/2 controller (port 0x60).
fn write_data(b: u8) {
    let mut p: Port<u8> = Port::new(PS2_DATA);
    wait_write();
    // SAFETY: 0x60 is the PS/2 data port.
    unsafe { p.write(b) };
}

/// Read a data byte from the PS/2 controller (port 0x60).
fn read_data() -> u8 {
    let mut p: Port<u8> = Port::new(PS2_DATA);
    wait_read();
    // SAFETY: 0x60 is the PS/2 data port.
    unsafe { p.read() }
}

/// Send a command to the mouse (auxiliary device) and return its ACK byte.
/// The 0xD4 prefix routes the next data byte to the aux device.
fn mouse_cmd(b: u8) -> u8 {
    cmd(0xD4);
    write_data(b);
    read_data()
}

/// Initialise the PS/2 mouse and wire IRQ12 through the IOAPIC.
///
/// Sequence: enable aux device, enable the aux IRQ in the controller config
/// byte, set mouse defaults, enable data reporting. Then route IRQ12 →
/// VEC_MOUSE. Logs ACK results so a non-responding device is visible at boot.
pub fn init(overrides: &[IrqOverride]) {
    // 1. Enable the auxiliary (mouse) PS/2 device.
    cmd(0xA8);

    // 2. Read the controller config byte, enable aux IRQ (bit1) and aux clock
    //    (clear bit5), then write it back.
    cmd(0x20);
    let mut config = read_data();
    config |= 0x02;   // enable IRQ12 (aux interrupt)
    config &= !0x20;  // enable aux clock (0 = enabled)
    cmd(0x60);
    write_data(config);

    // 3. Set defaults (0xF6) and 4. enable data reporting (0xF4). 0xFA = ACK.
    let ack_def = mouse_cmd(0xF6);
    let ack_en = mouse_cmd(0xF4);
    crate::binfo!(
        "mouse", "init defaults_ack=0x{:02X} enable_ack=0x{:02X}",
        ack_def, ack_en
    );

    // 5. Route IRQ12 to the mouse vector (handles ACPI interrupt overrides,
    //    same as the keyboard's IRQ1 wiring).
    apic::ioapic::redirect(12, crate::idt::VEC_MOUSE, overrides);
}
```

- [ ] **Step 2: Call `init` from the interrupts boot phase**

In `kernel/src/boot/phases/interrupts.rs`, after the keyboard wiring (line 41-42)
and BEFORE the `#[cfg(feature = "boot-checks")]` self-test block added in Task 1,
add:

```rust
    crate::mouse::init(&acpi.overrides);
    crate::binfo!("intr", "mouse IRQ12 wired overrides={}", acpi.overrides.len());
```

Note: `init` must run while interrupts are still masked-then-enabled like the
keyboard — it is already before the `interrupts::enable()` call at line 45, which
is correct.

- [ ] **Step 3: Run to verify the boot sequence is healthy**

Run:
`wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && make test-boot'`
Expected: log shows `mouse: init defaults_ack=0xFA enable_ack=0xFA`,
`intr: mouse IRQ12 wired ...`, `mouse: decode self-test ok`, and `$(HELLO)`.
If the ACKs are not `0xFA`, the device did not respond — see Task 6 for the QEMU
flag needed to attach a mouse.

- [ ] **Step 4: Commit**

```bash
git add kernel/src/mouse/mod.rs kernel/src/boot/phases/interrupts.rs
git commit -m "feat(mouse): PS/2 controller enable sequence + IRQ12 IOAPIC wiring"
```

---

## Task 6: Live IRQ integration check in QEMU

**Files:**
- Modify: `kernel/src/boot/phases/interrupts.rs` (boot-check only)

QEMU's q35 default does not always expose a PS/2 mouse; add an explicit device
and inject motion so a real IRQ12 round-trip is observable.

- [ ] **Step 1: Add a deferred "saw a real event" boot-check log**

A mouse event only arrives after boot, asynchronously. Add a lightweight
diagnostic the smoke phase can observe: in `kernel/src/mouse/mod.rs`, add a
counter incremented by the ISR and a helper to read it:

```rust
/// Total events enqueued since boot (diagnostic; lets a smoke test confirm a
/// real IRQ12 round-trip happened).
static EVENT_COUNT: AtomicU32 = AtomicU32::new(0);

/// Number of mouse events seen since boot.
pub fn event_count() -> u32 {
    EVENT_COUNT.load(Ordering::Relaxed)
}
```

In `mouse_handler`, in the `_ =>` arm right after `push_event(ev);`, add:

```rust
            EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
```

- [ ] **Step 2: Run with a mouse device attached and inject motion**

Run the standard interactive image but with a PS/2 mouse and scripted motion via
the QEMU monitor. From the repo root via WSL:

```bash
wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && make iso CARGO_FEATURES=boot-checks'
wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && \
  printf "mouse_move 10 10\nmouse_move -5 -5\n" | \
  timeout 30 qemu-system-x86_64 -machine q35 -cpu max -boot d \
    -cdrom build/ruos.iso -serial stdio -display none -no-reboot -m 512 \
    -device qemu-xhci -monitor stdin'
```

Expected: the controller ACKs are `0xFA` in the serial log. (`mouse_move` over
the monitor drives the emulated PS/2 device; events increment `event_count()`.)

- [ ] **Step 3: Add a shell-visible probe (optional but recommended)**

So the round-trip is assertable without the monitor, expose `event_count()` via
the existing `dmesg`/log path: in the interrupts phase boot-check block (Task 1),
this is boot-time only and will read 0. Instead, log it from the periodic timer
is overkill — keep verification manual for v1 and note it:

Add a one-line comment above the Task 1 self-test block:

```rust
    // Live IRQ12 verification is manual (QEMU `mouse_move` over -monitor); see
    // docs/superpowers/plans/2026-06-04-mouse-ps2-driver.md Task 6.
```

- [ ] **Step 4: Commit**

```bash
git add kernel/src/mouse/mod.rs kernel/src/boot/phases/interrupts.rs
git commit -m "feat(mouse): boot event counter + documented live IRQ12 check"
```

---

## Task 7: Changelog entry

**Files:**
- Create: `CHANGELOG/NN-26-06-04-mouse-ps2-driver.md` (use the next free `NN`)

- [ ] **Step 1: Determine the next changelog number**

Run:
`ls CHANGELOG/ | sed 's/-.*//' | sort -n | tail -1`
Use that number + 1, zero-padded to 2 digits, as `NN`.

- [ ] **Step 2: Write the entry**

Create `CHANGELOG/NN-26-06-04-mouse-ps2-driver.md`:

```markdown
# NN — Driver mouse PS/2 (IRQ12)

**Data:** 2026-06-04

## Cosa
Aggiunto driver mouse PS/2: decoder pacchetti 3-byte (puro, self-test
boot-checks), coda eventi IRQ-safe (`pop_event`), ISR IRQ12 con assemblatore
pacchetti, sequenza init controller + wiring IOAPIC. Nuovo vettore IDT 0x22.
Primo prerequisito del desktop egui (input).

## Perché
La GUI egui (spec 2026-06-04-egui-desktop-wasmtime-aot) richiede input mouse.
Step 1 della serie di piani; indipendente, testabile da solo.

## File toccati
- kernel/src/mouse/mod.rs
- kernel/src/idt.rs
- kernel/src/boot/phases/interrupts.rs
- kernel/src/main.rs
```

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG/NN-26-06-04-mouse-ps2-driver.md
git commit -m "docs(changelog): mouse PS/2 driver entry"
```

---

## Self-Review notes (already applied)

- **Spec coverage:** implements §13 prereq #1 (mouse PS/2 → input queue) and the
  §10 testing item "Mouse PS/2: unit decode + integration". The `pop_event`
  drain API is the seam the later input-coalescer / `gfx_poll_event` (plan #4)
  consumes — no GUI coupling here, by design.
- **Types:** `MouseEvent`, `decode_packet([u8;3]) -> MouseEvent`, `pop_event() ->
  Option<MouseEvent>`, `event_count() -> u32`, `mouse_handler`, `init(&[IrqOverride])`
  are used consistently across tasks and match the keyboard module's conventions.
- **Known fragility:** `IrqMutex::new` const-ness is verified in Task 2 Step 1
  with a documented `spin::Once` fallback. PS/2 ACK timeouts are bounded spins
  (no infinite hang) consistent with `wait_read`/`wait_write`.
```
