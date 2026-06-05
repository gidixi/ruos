# Compositor GATE Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Prove the ruos kernel can hold ≥2 **persistent reactor** wasm instances, call an exported `frame()` on each round-robin, read each app's committed surface buffer, and **composite two windows side-by-side** on the framebuffer (both visibly updating) — de-risking the core unknown of the kernel-side multi-window compositor (spec `2026-06-05-multi-window-compositor-design.md`, §5).

**Architecture:** A tiny no_std reactor guest (`tools/wt-reactor`) exports `frame()` and imports a raw `wm` host module (`commit`, `app-id`, `tick`); it draws a solid color (cycling per frame, offset by its id) into a static buffer in its own linear memory and `commit`s it. The kernel keeps N `(Store<WmState>, Instance)` alive, calls `frame()` on each per loop, copies each committed buffer into that store's `WmState.pixels`, then blits each to its window rect (reusing `crate::gfx::blit`). Raw `extern` imports (not WIT) keep the spike focused on the concurrency mechanism; WIT-ification comes when building the real compositor.

**Tech Stack:** Rust pinned nightly; guest `wasm32-unknown-unknown` (no_std, no WASI); kernel wasmtime 45 **core** `Module`/`Linker`/`Instance` (persistent instances + repeated `TypedFunc` calls); `wt-precompile` (precompile_module). Built via WSL `make`. Verification = boot-check markers (mechanism) + QEMU+KVM screendump (visual).

---

## File Structure
- `tools/wt-reactor/{Cargo.toml, src/lib.rs}` — **Create.** no_std `wasm32-unknown-unknown` guest: exports `frame`, imports `wm.{commit,app_id,tick}`, draws a cycling color into a static buffer + commits.
- `kernel/src/wasm/wt/wm.rs` — **Create.** `WmState` (per-instance store data: id, win w/h, committed pixels, tick) + `add_to_linker` for the `wm` host module + `run_reactor_spike()` (Tasks 1-2) and `run_compositor_gate()` (Task 3).
- `kernel/src/wasm/wt/mod.rs` — **Modify.** `pub mod wm;` + embed `reactor.cwasm` + boot-check demos.
- `kernel/src/boot/phases/interrupts.rs` — **Modify.** Wire the boot-check markers (Tasks 1-2).
- `kernel/src/wasm/wt/reactor.cwasm` — generated (gitignored) build artifact (Makefile rule).
- `Makefile` — **Modify.** Build the reactor guest + precompile → `reactor.cwasm`; a `compositor` launch path for the visual gate.

---

## Task 1: Reactor mechanism spike (THE core risk)

Prove the kernel can instantiate a wasm module and call an exported `frame()` **repeatedly** on a **persistent** instance.

**Files:** Create `tools/wt-reactor/{Cargo.toml, src/lib.rs}`; create `kernel/src/wasm/wt/wm.rs`; modify `kernel/src/wasm/wt/mod.rs`, `kernel/src/boot/phases/interrupts.rs`.

- [ ] **Step 1: Guest crate** — `tools/wt-reactor/Cargo.toml`:
```toml
[package]
name = "wt-reactor"
version = "0.0.0"
edition = "2021"
[lib]
crate-type = ["cdylib"]
[profile.release]
panic = "abort"
lto = true
```

- [ ] **Step 2: Guest `tools/wt-reactor/src/lib.rs`** (no allocator needed — static buffer only):
```rust
#![no_std]

#[link(wasm_import_module = "wm")]
extern "C" {
    fn commit(ptr: *const u8, len: u32, w: u32, h: u32);
    fn app_id() -> u32;
    fn tick();
}

const W: usize = 320;
const H: usize = 240;
static mut BUF: [u8; W * H * 4] = [0; W * H * 4];
static mut COUNTER: u32 = 0;

#[no_mangle]
pub extern "C" fn frame() {
    unsafe {
        tick();
        COUNTER = COUNTER.wrapping_add(1);
        let id = app_id();
        let r = (COUNTER.wrapping_add(id.wrapping_mul(80)) & 0xff) as u8;
        let p = core::ptr::addr_of_mut!(BUF) as *mut u8;
        let mut i = 0;
        while i < W * H * 4 {
            *p.add(i) = r; *p.add(i + 1) = 0x40; *p.add(i + 2) = 0x80; *p.add(i + 3) = 0xff;
            i += 4;
        }
        commit(core::ptr::addr_of!(BUF) as *const u8, (W * H * 4) as u32, W as u32, H as u32);
    }
}

#[panic_handler]
fn ph(_: &core::panic::PanicInfo) -> ! { loop {} }
```

- [ ] **Step 3: Install the target + build the guest + precompile** (WSL):
```bash
wsl -d Ubuntu -u root -e bash -lc 'source $HOME/.cargo/env && rustup target add wasm32-unknown-unknown 2>&1 | tail -1 && cd /mnt/e/MinimalOS/BasicOperatingSystem && (cd tools/wt-reactor && cargo build --release --target wasm32-unknown-unknown 2>&1 | tail -8) && wasm-tools print tools/wt-reactor/target/wasm32-unknown-unknown/release/wt_reactor.wasm | grep -E "import \"wm\"|export.*frame" && tools/wt-precompile/target/release/wt-precompile tools/wt-reactor/target/wasm32-unknown-unknown/release/wt_reactor.wasm kernel/src/wasm/wt/reactor.cwasm 2>&1 | tail -2'
```
Expected: imports `wm.commit/app_id/tick`, exports `frame`; `wrote ...reactor.cwasm`.

- [ ] **Step 4: Kernel `kernel/src/wasm/wt/wm.rs`** — store data + host module + the spike runner:
```rust
//! Window-manager / compositor host module (`wm`) + reactor driver. Holds N
//! persistent wasm instances; calls their exported `frame()` round-robin; reads
//! each committed surface into the per-store WmState.

use alloc::vec::Vec;
use wasmtime::{Caller, Linker, Module, Store};
use crate::wasm::wt::{engine, mem};

/// Per-instance store data: window id + last committed surface.
pub struct WmState {
    pub id: u32,
    pub win_w: u32,
    pub win_h: u32,
    pub pixels: Vec<u8>,
    pub tick: u32,
}

pub fn add_to_linker(linker: &mut Linker<WmState>) -> wasmtime::Result<()> {
    // wm.commit(ptr, len, w, h): copy the guest's surface into WmState.pixels.
    linker.func_wrap("wm", "commit",
        |mut caller: Caller<'_, WmState>, ptr: i32, len: i32, w: i32, h: i32| {
            if let Some(b) = mem::read(&mut caller, ptr as u32, len as u32) {
                let s = caller.data_mut();
                s.pixels = b;
                s.win_w = w as u32;
                s.win_h = h as u32;
            }
        })?;
    // wm.app-id() -> u32: this instance's window id.
    linker.func_wrap("wm", "app-id",
        |caller: Caller<'_, WmState>| -> i32 { caller.data().id as i32 })?;
    // wm.tick(): bump the call counter (spike instrumentation).
    linker.func_wrap("wm", "tick",
        |mut caller: Caller<'_, WmState>| { caller.data_mut().tick += 1; })?;
    Ok(())
}

/// SPIKE: instantiate ONE reactor instance, call `frame()` 5× on it, return the
/// tick count. Proves a persistent instance + repeated export call.
pub fn run_reactor_spike(cwasm: &[u8]) -> u32 {
    let engine = engine();
    // SAFETY: produced by wt-precompile for this exact engine Config.
    let module = match unsafe { Module::deserialize(engine, cwasm) } {
        Ok(m) => m, Err(_) => return 0,
    };
    let mut store = Store::new(engine, WmState { id: 0, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0 });
    let mut linker: Linker<WmState> = Linker::new(engine);
    if add_to_linker(&mut linker).is_err() { return 0; }
    #[cfg(target_arch = "x86_64")]
    unsafe { core::arch::asm!("cld", options(nostack)); }
    let instance = match linker.instantiate(&mut store, &module) { Ok(i) => i, Err(_) => return 0 };
    let frame = match instance.get_typed_func::<(), ()>(&mut store, "frame") { Ok(f) => f, Err(_) => return 0 };
    for _ in 0..5 {
        if frame.call(&mut store, ()).is_err() { break; }
    }
    store.data().tick
}
```

- [ ] **Step 5: Wire in `kernel/src/wasm/wt/mod.rs`** — `pub mod wm;` + embed + demo:
```rust
#[cfg(feature = "boot-checks")]
static REACTOR_CWASM: &[u8] = include_bytes!("reactor.cwasm");

/// Boot self-test: a reactor instance whose frame() is called 5× → tick==5.
#[cfg(feature = "boot-checks")]
pub fn run_reactor_spike_demo() -> u32 {
    crate::wasm::wt::wm::run_reactor_spike(REACTOR_CWASM)
}
```

- [ ] **Step 6: Boot-check marker** in `kernel/src/boot/phases/interrupts.rs` (inside the `#[cfg(feature="boot-checks")]` block, after the component bring-up line):
```rust
        let rt = crate::wasm::wt::run_reactor_spike_demo();
        crate::binfo!("wm", "reactor spike frame-calls={}", rt);
```

- [ ] **Step 7: Makefile** — add a rule to build the reactor guest + precompile (mirror the `bringup.cwasm` rule), and add `kernel/src/wasm/wt/reactor.cwasm` to the `test-boot` prerequisites:
```makefile
kernel/src/wasm/wt/reactor.cwasm: tools/wt-reactor/src/lib.rs tools/wt-reactor/Cargo.toml $(WT_PRECOMPILE)
	@mkdir -p build
	source $$HOME/.cargo/env && cd tools/wt-reactor && \
		cargo build --release --target wasm32-unknown-unknown
	$(WT_PRECOMPILE) tools/wt-reactor/target/wasm32-unknown-unknown/release/wt_reactor.wasm kernel/src/wasm/wt/reactor.cwasm
```

- [ ] **Step 8: Run the boot test + assert** (scratch ISO):
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot ISO=build/cmtest.iso 2>&1 | tail -8'
```
Then: `grep -E "wm   reactor spike frame-calls=5" build/test-boot.log` → must match (frame() was called 5× on a persistent instance). Anything other than 5 = the reactor mechanism failed; report the value + serial.

- [ ] **Step 9: Commit** (NO changelog — controller consolidates):
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add tools/wt-reactor kernel/src/wasm/wt/wm.rs kernel/src/wasm/wt/mod.rs kernel/src/boot/phases/interrupts.rs Makefile && git commit -m "feat(wm): reactor instance spike (persistent instance, repeated frame() call)"
```

---

## Task 2: Surface commit reaches the kernel

Prove a committed surface buffer arrives in `WmState.pixels` with the right content.

**Files:** Modify `kernel/src/wasm/wt/wm.rs`, `kernel/src/wasm/wt/mod.rs`, `kernel/src/boot/phases/interrupts.rs`.

- [ ] **Step 1: Extend the spike to check the committed buffer.** In `wm.rs`, after the 5× `frame()` loop in `run_reactor_spike`, the store already has `pixels` (the guest committed each frame). Add a second return path — change the demo to also expose a pixel check. Simplest: add a function:
```rust
/// Returns (frame_calls, first_pixel_byte0, pixel_len) after the spike.
pub fn run_reactor_spike2(cwasm: &[u8]) -> (u32, u8, usize) {
    // identical setup to run_reactor_spike ... after the loop:
    //   let s = store.data();
    //   let b0 = s.pixels.first().copied().unwrap_or(0);
    //   (s.tick, b0, s.pixels.len())
}
```
(Copy `run_reactor_spike`'s body; return `(store.data().tick, store.data().pixels.first().copied().unwrap_or(0), store.data().pixels.len())`.)

- [ ] **Step 2: mod.rs demo + boot marker.** Replace the Task-1 demo call with one that prints the committed-pixel proof:
```rust
#[cfg(feature = "boot-checks")]
pub fn run_reactor_spike_demo() -> (u32, u8, usize) {
    crate::wasm::wt::wm::run_reactor_spike2(REACTOR_CWASM)
}
```
interrupts.rs:
```rust
        let (calls, b0, plen) = crate::wasm::wt::run_reactor_spike_demo();
        crate::binfo!("wm", "reactor spike calls={} commit_b0=0x{:02X} pixels={}", calls, b0, plen);
```
The guest fills byte0 = `COUNTER+id*80` & 0xff; after 5 frames with id=0, COUNTER=5 → byte0 should be 5. `pixels` should be 320*240*4 = 307200.

- [ ] **Step 3: Boot test + assert:**
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot ISO=build/cmtest.iso 2>&1 | tail -6'
```
`grep -E "reactor spike calls=5 commit_b0=0x05 pixels=307200" build/test-boot.log` → must match. Proves the surface commit (guest buffer → kernel) works.

- [ ] **Step 4: Commit:**
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/wm.rs kernel/src/wasm/wt/mod.rs kernel/src/boot/phases/interrupts.rs && git commit -m "feat(wm): verify surface commit reaches the kernel"
```

---

## Task 3: Two instances composited side-by-side (the visual gate)

**Files:** Modify `kernel/src/wasm/wt/wm.rs`, and a launch path.

- [ ] **Step 1: `run_compositor_gate` in `wm.rs`** — 2 persistent instances, round-robin `frame()`, blit each to its rect:
```rust
/// Visual GATE: 2 reactor instances, side-by-side, both updating. Owns the CPU
/// (like the single-GUI path today). Never returns.
pub fn run_compositor_gate(cwasm: &[u8]) -> ! {
    crate::gfx::enter();
    let engine = engine();
    let module = unsafe { Module::deserialize(engine, cwasm) }.expect("reactor module");
    let mut linker: Linker<WmState> = Linker::new(engine);
    add_to_linker(&mut linker).expect("wm linker");
    #[cfg(target_arch = "x86_64")]
    unsafe { core::arch::asm!("cld", options(nostack)); }

    // Two windows: left and right halves of the screen (rects).
    let g = crate::gfx::geom();
    let rects = [(0u32, 0u32), (g.width / 2, 0u32)]; // (x,y) origin of each window
    let mut wins: Vec<(Store<WmState>, wasmtime::Instance, (u32, u32))> = Vec::new();
    for (id, &origin) in rects.iter().enumerate() {
        let mut store = Store::new(engine, WmState { id: id as u32, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0 });
        let inst = linker.instantiate(&mut store, &module).expect("instantiate");
        wins.push((store, inst, origin));
    }
    loop {
        for (store, inst, origin) in wins.iter_mut() {
            if let Ok(frame) = inst.get_typed_func::<(), ()>(&mut *store, "frame") {
                let _ = frame.call(&mut *store, ());
            }
            let s = store.data();
            if !s.pixels.is_empty() {
                crate::gfx::blit(&s.pixels, origin.0, origin.1, s.win_w, s.win_h);
            }
        }
        // crude pacing so the colour cycle is visible.
        for _ in 0..2_000_000 { core::hint::spin_loop(); }
    }
}
```
(`crate::gfx::blit` already clips + composites the cursor; blitting two 320×240 buffers at (0,0) and (w/2,0) gives two side-by-side windows.)

- [ ] **Step 2: Launch path.** Add a `compositor` entry: in `kernel/src/wasm/wt/mod.rs` embed the reactor cwasm unconditionally (not just boot-checks) and add `pub fn run_compositor_gate_demo() -> !`; wire a way to start it — simplest: a new init/command. Reuse the existing GUI launch detection in `executor/mod.rs` (the `.cwasm` router): make a special path name `compositor` route to `wm::run_compositor_gate`. Concretely, in `executor/mod.rs` where it routes `.cwasm`, add: `if slot.path.ends_with("compositor") { crate::wasm::wt::wm::run_compositor_gate(REACTOR_CWASM_STATIC) }`. (Embed the cwasm in a non-boot-checks static for this.) Then an init script runs `compositor`.
  - Create `user-bin/compositor-init.sh`: `echo ruos boot OK` + `compositor`.

- [ ] **Step 3: Build a GUI-style ISO + screendump:**
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso INIT_SCRIPT=user-bin/compositor-init.sh ISO=build/comptest.iso 2>&1 | tail -3'
```
Then boot QEMU+KVM with QMP and run `build/shot.py` (existing helper) to capture `build/shot.png` after ~14s.

- [ ] **Step 4: Inspect the screendump.** `build/shot.png` must show **TWO solid-colour rectangles side-by-side** (left at 0,0; right at width/2,0), each 320×240, with **different** colours (the id offset). Take two shots a second apart → the colours must have **changed** (frame() is being called each loop). This proves: 2 persistent instances + round-robin frame() + per-app surface + compositing.

- [ ] **Step 5: Commit:**
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/wm.rs kernel/src/wasm/wt/mod.rs kernel/src/executor/mod.rs user-bin/compositor-init.sh && git commit -m "feat(wm): compositor gate — two reactor windows side-by-side"
```

---

## Task 4: Changelog + final review

- [ ] **Step 1:** Write `CHANGELOG/NN-...` (next free number) summarizing the gate: reactor instances (persistent, frame() round-robin), surface commit, 2-window compositing; QEMU-verified (2 updating windows). Note this de-risks the multi-window compositor (spec 276).
- [ ] **Step 2:** Commit the changelog. Dispatch a final code-reviewer over `kernel/src/wasm/wt/wm.rs` + the guest.

---

## Follow-on (NOT in this plan)
Once the gate is green: sub-project 2 (input + focus routing), 3 (window manager: drag/resize/z-order/decorations), 4 (SMP-parallel compositing via the compute pool), 5 (launcher/lifecycle). Each: spec→plan→build. The real apps will use the WIT `surface` interface (vs the raw `wm` imports of this gate).

## Self-Review notes
- **Spec coverage:** implements spec §5 (the GATE) + §3.1 (reactor model) + §3.2 (surface commit) + §3.3 (minimal compositing). §3.4 input routing, §4 sub-projects 2-5, §6 SMP are explicitly deferred to follow-on plans.
- **Placeholders:** none — guest + kernel code is concrete. Task 2 Step 1 says "copy run_reactor_spike's body" with the exact return expression given (not a vague TODO).
- **Consistency:** `WmState{id,win_w,win_h,pixels,tick}`, the `wm.{commit,app-id,tick}` import names, and the `frame` export are used identically across guest, host, and the spike/gate runners. Marker strings (`reactor spike calls=5 commit_b0=0x05 pixels=307200`) match the guest's computed values (id=0, 5 frames → byte0=5, 320*240*4=307200).
- **Risk:** the one unknown is Task 1 (persistent instance + repeated `frame()`); it is isolated as the first task with a precise boot-check assertion. If it fails, STOP — the whole compositor direction depends on it.
