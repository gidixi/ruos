> **⚠️ READ THE INTERFACE CONTRACT FIRST:** `2026-06-05-compositor-subprojects-interface-contract.md` — it is AUTHORITATIVE over this draft. **SP2 must build the canonical `struct Window` + `struct Compositor` (+ `new/run/window_at/set_focus/raise/frame_all`) in its TASK BODY** (this draft's "Provides" promises them but the draft code keeps the gate's tuple `Vec` — fix that). Keep the entry name `run_compositor_gate` (executor calls it). Add `crate::gfx::mouse_pos()` as the single cursor source. Surface pixels stay in `store.data().pixels`; z-order = `wins` Vec order (no `z` field).

# Compositor SP2 — Input + Focus Routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Turn the static 2-window compositor GATE (`run_compositor_gate`) into a real input-routed multi-window loop: each loop drain the kernel's gfx event queue, hit-test the cursor against each window's fixed rect, set the **focused** window on a mouse-button-down (click-to-focus), translate mouse coordinates to window-local, and push events into **only the focused window's** per-window queue. A new `wm.poll_event` host fn lets each app drain ITS OWN window's queue (the app is identified by its `Store`'s `id`). The compositor draws a **focus border** around the active window. The reactor guest is extended to `poll_event` and maintain a per-window CLICK COUNTER it draws into its surface, so only the focused window's counter rises. (Spec `2026-06-05-multi-window-compositor-design.md`, §3.4.)

**Architecture:** Input stays kernel-owned (`crate::gfx`): PS/2 mouse + keyboard are already coalesced into one `GfxEvt` queue drained by `crate::gfx::pop()`. In compositor mode `run_compositor_gate` is the SOLE consumer: each loop it calls `crate::gfx::fold_mouse()` (folds PS/2 deltas → absolute cursor + emits move/button events), then drains `crate::gfx::pop()`. For each drained event it routes by the latest cursor position (`crate::gfx::mouse_pos()`, a new getter): a `mousebtn` press hit-tested inside a window's rect sets that window as focused (raises = z-order is SP3, so "raised" here = focused); mouse-move/btn events get coordinates translated window-local and key events pass through, both pushed into the focused window's `WmState.events` `VecDeque`. Each app drains its own queue via `wm.poll_event` (a typed host fn lowering an `option<gfx-event>` into a guest return area, exactly like `ruos:gui/gfx poll-event`). The compositor blits each window's committed surface then draws a 2px border around the focused window. Windows stay at FIXED rects (drag/resize = SP3).

**Tech Stack:** Rust pinned nightly; guest `wasm32-unknown-unknown` (no_std, no WASI, static buffers — no allocator); kernel wasmtime 45 **core** `Module`/`Linker`/`Instance` (persistent instances, `WmState` store data) + `func_wrap` host fns reading/writing guest linear memory via the private `read_guest` helper in `wm.rs`. Built via WSL `make` (build-std, `x86_64-unknown-none`). Verification = boot-check markers (mechanism) + QEMU+KVM QMP screendump with `input-send-event` mouse clicks (visual focus + per-window counter).

---

## Assumed interfaces from prior sub-projects (SP-GATE, on main — CHANGELOG 277)

These are CONCRETE and verified in the current tree; SP2 builds directly on them.

- `kernel/src/wasm/wt/wm.rs`:
  - `pub struct WmState { pub id: u32, pub win_w: u32, pub win_h: u32, pub pixels: Vec<u8>, pub tick: u32 }`
  - `fn read_guest(caller: &mut Caller<'_, WmState>, ptr: u32, len: u32) -> Option<Vec<u8>>` (private; reads `WmState` guest mem — `wt::mem` is typed to `WtState` so cannot be reused).
  - `pub fn add_to_linker(linker: &mut Linker<WmState>) -> wasmtime::Result<()>` — host module `wm` with `commit(ptr,len,w,h)`, `app_id()->i32`, `tick()`.
  - `pub fn run_compositor_gate(cwasm: &[u8]) -> !` — owns the CPU, never returns; 2 persistent `(Store<WmState>, Instance, origin)` at origins `(0,0)` and `(g.width/2, 0)`, round-robin `frame()`, blit each `WmState.pixels` to its origin. **This is the function SP2 rewrites into the input loop.**
- `kernel/src/gfx/mod.rs`:
  - `pub struct GfxEvt { pub kind: u32, pub p0: u32, pub p1: u32, pub p2: u32 }` (kind 0=key{p0=scancode,p1=pressed}, 1=mousemove{p0=x f32 bits,p1=y f32 bits}, 2=mousebtn{p0=button 0L/1R/2M,p1=pressed}, 3=resize, 4=quit).
  - `pub fn fold_mouse()` — folds PS/2 mouse into absolute `MOUSE_X`/`MOUSE_Y` (atomics, currently **private**) + emits move/button `GfxEvt`s + moves the software cursor.
  - `pub fn pop() -> Option<GfxEvt>`, `pub fn pending() -> usize`.
  - `pub fn blit(buf: &[u8], x: u32, y: u32, w: u32, h: u32)` — RGBA8888 rect, clips to screen, recomposites the software cursor.
  - `pub fn geom() -> GfxGeom { width, height, stride, format }`, `pub fn enter()`.
  - `MOUSE_X: AtomicI32`, `MOUSE_Y: AtomicI32` are **private** — SP2 adds a public `mouse_pos()` getter (Task 1).
- `kernel/src/executor/mod.rs`: `.cwasm` exec router special-cases `if slot.path.ends_with("compositor.cwasm") { run_compositor_gate(&bytes) }`. Launch chain: `user-bin/compositor-init.sh` runs `compositor` → `/bin/compositor.cwasm` (the reactor cwasm, a Limine boot module). **SP2 does not touch the executor.**
- `tools/wt-reactor/src/lib.rs`: no_std `wasm32-unknown-unknown`, exports `#[no_mangle] frame()`, imports `#[link(wasm_import_module="wm")] extern "C" { commit; app_id; tick; }`, static `BUF` (320×240×4) + `COUNTER`. Built + precompiled to `kernel/src/wasm/wt/reactor.cwasm` (gitignored) by the Makefile rule `kernel/src/wasm/wt/reactor.cwasm:`.

## File Structure
- `kernel/src/gfx/mod.rs` — **Modify.** Add `pub fn mouse_pos() -> (i32, i32)` (read `MOUSE_X`/`MOUSE_Y`).
- `kernel/src/wasm/wt/wm.rs` — **Modify.** Add `events: VecDeque<GfxEvt>` to `WmState`; add `wm.poll_event` host fn (drains the calling store's queue into a guest return area); add focus state + hit-test + routing + focus-border drawing inside `run_compositor_gate`.
- `tools/wt-reactor/src/lib.rs` — **Modify.** Import `poll_event`; each `frame()` drain events, increment a per-window `CLICKS` static on a left-button-down, and draw `CLICKS` (a filled bar whose width = clicks) into the surface so only the focused window's counter visibly rises.
- `kernel/src/wasm/wt/reactor.cwasm` — generated (gitignored) build artifact (existing Makefile rule rebuilds it from the modified guest).

---

## Task 1: Expose the cursor position from `crate::gfx`

The compositor must hit-test the live cursor against window rects, but `MOUSE_X`/`MOUSE_Y` are private statics. Add a public getter.

**Files:** Modify `kernel/src/gfx/mod.rs`.

- [ ] **Step 1: Add `mouse_pos()` to `kernel/src/gfx/mod.rs`.** Insert this directly after the `pub fn pending()` function (around line 210, before the `static MOUSE_X` declaration is fine — Rust items are order-independent; place it after `pending()` for readability):
```rust
/// Current absolute software-cursor position (x, y) in framebuffer pixels.
/// The compositor hit-tests this against window rects to route input. Updated
/// by `fold_mouse()` from the PS/2 mouse.
pub fn mouse_pos() -> (i32, i32) {
    (MOUSE_X.load(Ordering::Relaxed), MOUSE_Y.load(Ordering::Relaxed))
}
```

- [ ] **Step 2: Verify it compiles** (kernel build via WSL; build-std, target `x86_64-unknown-none`):
```bash
wsl -d Ubuntu -u root -e bash -lc 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem/kernel && cargo build --release -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem --target x86_64-unknown-none 2>&1 | tail -5'
```
Expected: `Finished` (no errors). `mouse_pos` is `pub` so an unused-warning is acceptable until Task 3 uses it.

- [ ] **Step 3: Commit** (NO changelog — controller consolidates):
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/gfx/mod.rs && git commit -m "feat(gfx): expose mouse_pos() for compositor hit-testing"
```

---

## Task 2: Per-window event queue + `wm.poll_event` host fn

Give each window its own input queue and a host fn the app calls to drain ITS OWN queue. The app is identified implicitly: `poll_event`'s `Caller` is the calling store, so it drains `caller.data_mut().events` — no app needs to pass its id.

**Files:** Modify `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: Extend `WmState` with a per-window event queue.** In `kernel/src/wasm/wt/wm.rs`, change the imports + struct. Replace the existing `use alloc::vec::Vec;` line with:
```rust
use alloc::vec::Vec;
use alloc::collections::VecDeque;
use crate::gfx::GfxEvt;
```
Replace the `WmState` struct (currently 5 fields) with:
```rust
/// Per-instance store data: window id, last committed surface, and this window's
/// private input queue (the compositor pushes routed events here; the app drains
/// them via `wm.poll_event`).
pub struct WmState {
    pub id: u32,
    pub win_w: u32,
    pub win_h: u32,
    pub pixels: Vec<u8>,
    pub tick: u32,
    pub events: VecDeque<GfxEvt>,
}
```

- [ ] **Step 2: Initialise `events` at every `WmState` construction site.** There are TWO in `wm.rs` (one in `run_reactor_spike`, one in `run_compositor_gate`). Each currently reads:
```rust
WmState { id: 0, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0 }
```
(or `id: id as u32` in the gate). Add `events: VecDeque::new()` to BOTH:
  - In `run_reactor_spike` (the boot-check spike), change to:
```rust
    let mut store = Store::new(engine, WmState { id: 0, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0, events: VecDeque::new() });
```
  - In `run_compositor_gate`, change the per-window construction (inside the `for (id, &origin)` loop) to:
```rust
        let mut store = Store::new(
            engine,
            WmState { id: id as u32, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0, events: VecDeque::new() },
        );
```

- [ ] **Step 3: Add the `wm.poll_event` host fn.** In `add_to_linker`, after the existing `wm.tick` `func_wrap` and before `Ok(())`, add:
```rust
    // wm.poll_event(retptr): drain ONE event from THIS window's queue into the
    // guest's 20-byte return area. The calling app is identified by its own
    // Store (caller.data()), so it can only ever see its own window's events.
    // Layout matches `ruos:gui/gfx poll-event`: discriminant i32 @0 (0=none,
    // 1=some), then the gfx-event record kind@4, p0@8, p1@12, p2@16.
    linker.func_wrap("wm", "poll_event",
        |mut caller: Caller<'_, WmState>, retptr: i32| {
            let ev = caller.data_mut().events.pop_front();
            let mut buf = [0u8; 20];
            if let Some(e) = ev {
                buf[0..4].copy_from_slice(&1u32.to_le_bytes());   // some
                buf[4..8].copy_from_slice(&e.kind.to_le_bytes());
                buf[8..12].copy_from_slice(&e.p0.to_le_bytes());
                buf[12..16].copy_from_slice(&e.p1.to_le_bytes());
                buf[16..20].copy_from_slice(&e.p2.to_le_bytes());
            }
            // else: discriminant stays 0 (none); payload zeroed.
            write_guest(&mut caller, retptr as u32, &buf);
        })?;
```

- [ ] **Step 4: Add a `write_guest` helper.** `read_guest` exists; add its write twin directly below `read_guest` in `wm.rs`:
```rust
/// Write `buf` into this guest's exported linear memory at `ptr`. No-op (returns
/// false) if the memory export is missing or the range is out of bounds. (Mirrors
/// `crate::wasm::wt::mem::write`, which is typed to `WtState`.)
fn write_guest(caller: &mut Caller<'_, WmState>, ptr: u32, buf: &[u8]) -> bool {
    let mem = match caller.get_export("memory") {
        Some(Extern::Memory(m)) => m,
        _ => return false,
    };
    let mem: Memory = mem;
    mem.write(caller, ptr as usize, buf).is_ok()
}
```
(`Extern` and `Memory` are already imported at the top of `wm.rs` for `read_guest`.)

- [ ] **Step 5: Verify it compiles** (the guest does not yet call `poll_event`, but a missing import is only an error at instantiate-time — adding the host fn is always safe; an unprovided host fn would be the failure, never an extra one):
```bash
wsl -d Ubuntu -u root -e bash -lc 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem/kernel && cargo build --release -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem --target x86_64-unknown-none 2>&1 | tail -5'
```
Expected: `Finished`.

- [ ] **Step 6: Commit:**
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/wm.rs && git commit -m "feat(wm): per-window event queue + wm.poll_event host fn"
```

---

## Task 3: Input routing + focus + focus border in `run_compositor_gate`

Rewrite the gate loop to drain the kernel event queue, hit-test the cursor, set focus on click, route events to the focused window's queue, and draw a focus border. Windows stay at FIXED rects.

**Files:** Modify `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: Add a window-rect helper + focus-border drawer.** Add these two free functions to `wm.rs` (e.g. directly above `run_compositor_gate`). The window size is the reactor surface size (320×240); a window's rect is `(origin.0, origin.1, 320, 240)`.
```rust
/// Reactor surface size (matches `tools/wt-reactor` W/H). Windows are fixed at
/// this size in SP2; SP3 will make them resizable.
const WIN_W: u32 = 320;
const WIN_H: u32 = 240;

/// True if framebuffer point (px,py) is inside the window at `origin`.
fn hit(origin: (u32, u32), px: i32, py: i32) -> bool {
    let (ox, oy) = (origin.0 as i32, origin.1 as i32);
    px >= ox && px < ox + WIN_W as i32 && py >= oy && py < oy + WIN_H as i32
}

/// Draw a `thick`-px solid border (RGBA `color`) just inside the window rect at
/// `origin`, so it sits over the app's committed surface. Uses tiny stack rows
/// blitted via `crate::gfx::blit` (clips to screen, recomposites the cursor).
fn draw_border(origin: (u32, u32), thick: u32, color: [u8; 4]) {
    let (ox, oy) = origin;
    // Horizontal strips (top + bottom): WIN_W wide, `thick` tall.
    let mut hrow = alloc::vec![0u8; (WIN_W * thick * 4) as usize];
    for px in hrow.chunks_mut(4) { px.copy_from_slice(&color); }
    crate::gfx::blit(&hrow, ox, oy, WIN_W, thick);
    crate::gfx::blit(&hrow, ox, oy + WIN_H - thick, WIN_W, thick);
    // Vertical strips (left + right): `thick` wide, WIN_H tall.
    let mut vrow = alloc::vec![0u8; (thick * WIN_H * 4) as usize];
    for px in vrow.chunks_mut(4) { px.copy_from_slice(&color); }
    crate::gfx::blit(&vrow, ox, oy, thick, WIN_H);
    crate::gfx::blit(&vrow, ox + WIN_W - thick, oy, thick, WIN_H);
}
```

- [ ] **Step 2: Replace `run_compositor_gate`'s body.** Replace the ENTIRE existing `run_compositor_gate` function (from `pub fn run_compositor_gate(cwasm: &[u8]) -> ! {` through its closing `}`) with the input-routed version below. It keeps the same setup (enter gfx, deserialize, linker, 2 windows at fixed origins) and adds: per-loop `fold_mouse()` + drain `pop()`, hit-test + click-to-focus, window-local coordinate translation, per-window queue push, then `frame()` round-robin, blit, and a focus border around the focused window.
```rust
/// SP2 GATE: 2 reactor windows with INPUT ROUTING + click-to-focus. Owns the CPU
/// (like the single-GUI path), never returns.
///
/// Each loop:
///  1. `fold_mouse()` (PS/2 -> absolute cursor + move/button GfxEvts), then drain
///     the kernel gfx event queue.
///  2. Route each event: a left-button-DOWN hit-tested inside a window focuses it
///     (click-to-focus). Mouse events get window-local coords; key events pass
///     through. The (possibly translated) event is pushed into ONLY the focused
///     window's per-window queue (`WmState.events`).
///  3. Call each window's `frame()` (it drains its queue via `wm.poll_event` and
///     redraws), blit its surface, then draw a focus border around the focused one.
///
/// Windows are FIXED rects (drag/resize = SP3).
pub fn run_compositor_gate(cwasm: &[u8]) -> ! {
    crate::kprintln!("[wm] compositor SP2: input + focus routing");
    crate::gfx::enter();
    let engine = engine();
    // SAFETY: produced by wt-precompile for this exact engine Config.
    let module = unsafe { Module::deserialize(engine, cwasm) }.expect("reactor module");
    let mut linker: Linker<WmState> = Linker::new(engine);
    add_to_linker(&mut linker).expect("wm linker");
    // SysV ABI requires DF=0; cranelift/Rust code uses `rep movs` which run
    // BACKWARD if DF=1, silently corrupting copied data.
    #[cfg(target_arch = "x86_64")]
    unsafe { core::arch::asm!("cld", options(nostack)); }

    // Two windows: left and right (fixed origins). Surfaces are WIN_W x WIN_H.
    let g = crate::gfx::geom();
    let origins = [(0u32, 0u32), (g.width / 2, 0u32)];
    let mut wins: Vec<(Store<WmState>, wasmtime::Instance, (u32, u32))> = Vec::new();
    for (id, &origin) in origins.iter().enumerate() {
        let mut store = Store::new(
            engine,
            WmState { id: id as u32, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0, events: VecDeque::new() },
        );
        let inst = linker.instantiate(&mut store, &module).expect("instantiate");
        wins.push((store, inst, origin));
    }
    // Window 0 starts focused (so input lands somewhere before the first click).
    let mut focused: usize = 0;

    loop {
        // 1. Input: fold PS/2 -> events, then drain the kernel queue.
        crate::gfx::fold_mouse();
        while let Some(ev) = crate::gfx::pop() {
            // Latest cursor position (kept up to date by fold_mouse).
            let (cx, cy) = crate::gfx::mouse_pos();
            // Click-to-focus: a LEFT-button DOWN inside a window focuses it.
            if ev.kind == 2 && ev.p0 == 0 && ev.p1 == 1 {
                for (i, (_s, _inst, origin)) in wins.iter().enumerate() {
                    if hit(*origin, cx, cy) {
                        focused = i;
                        break;
                    }
                }
            }
            // Route to the focused window. Translate mouse coords to window-local;
            // pass key events through unchanged.
            let origin = wins[focused].2;
            let routed = match ev.kind {
                1 => {
                    // mousemove: p0/p1 are f32 bits of absolute x/y -> window-local.
                    let lx = (cx - origin.0 as i32).max(0) as f32;
                    let ly = (cy - origin.1 as i32).max(0) as f32;
                    GfxEvt { kind: 1, p0: lx.to_bits(), p1: ly.to_bits(), p2: 0 }
                }
                2 => {
                    // mousebtn: carry the window-local cursor in p2-unused; keep
                    // button(p0)+pressed(p1). (Apps that need the click point read
                    // the last mousemove; SP2's reactor only needs the down edge.)
                    GfxEvt { kind: 2, p0: ev.p0, p1: ev.p1, p2: 0 }
                }
                // key / resize / quit: pass through unchanged.
                _ => ev,
            };
            wins[focused].1; // (no-op: silence any borrow-order lint; see push below)
            wins[focused].0.data_mut().events.push_back(routed);
        }

        // 2. Drive each app's frame(), then blit its committed surface.
        for (store, inst, origin) in wins.iter_mut() {
            if let Ok(frame) = inst.get_typed_func::<(), ()>(&mut *store, "frame") {
                let _ = frame.call(&mut *store, ());
            }
            let s = store.data();
            if !s.pixels.is_empty() {
                crate::gfx::blit(&s.pixels, origin.0, origin.1, s.win_w, s.win_h);
            }
        }

        // 3. Focus border: 2px bright yellow inside the focused window's rect.
        draw_border(wins[focused].2, 2, [0xFF, 0xFF, 0x00, 0xFF]);

        // Crude pacing so the colour cycle + input feel responsive.
        for _ in 0..2_000_000u32 { core::hint::spin_loop(); }
    }
}
```

> Note: the `wins[focused].1;` no-op line in the draft above is NOT needed — DELETE it when transcribing. The borrow is fine because `wins[focused].0.data_mut()` takes a single mutable borrow of one element field. Final routed-push line is simply:
> ```rust
>             wins[focused].0.data_mut().events.push_back(routed);
> ```

- [ ] **Step 3: Verify it compiles:**
```bash
wsl -d Ubuntu -u root -e bash -lc 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem/kernel && cargo build --release -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem --target x86_64-unknown-none 2>&1 | tail -8'
```
Expected: `Finished`. If a borrow-checker error mentions `wins`, confirm the no-op line was deleted (Step 2 note).

- [ ] **Step 4: Commit:**
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/wm.rs && git commit -m "feat(wm): compositor input routing + click-to-focus + focus border"
```

---

## Task 4: Extend the reactor guest to react to input (per-window click counter)

The guest now drains its window's queue each `frame()` and increments a CLICK counter on a left-button-down, drawing the count as a filled bar so only the focused window's counter visibly rises.

**Files:** Modify `tools/wt-reactor/src/lib.rs`.

- [ ] **Step 1: Replace `tools/wt-reactor/src/lib.rs`** with the input-aware reactor. It keeps the cycling background (so you can see frames advancing) and overlays a vertical "tally bar" whose height = `CLICKS` rows of bright white at the left edge of the surface. The `poll_event` return area is a fixed 20-byte static read with the documented layout.
```rust
#![no_std]

//! Reactor guest for the compositor (SP2: input + focus). Exports `frame()` and
//! imports the raw `wm` host module (`commit`, `app_id`, `tick`, `poll_event`).
//! Each `frame()`: ticks the host, drains THIS window's event queue via
//! `poll_event`, increments a static CLICK counter on every left-button-down,
//! fills a static RGBA buffer with a per-frame cycling colour, then overlays a
//! white "tally bar" whose pixel height encodes CLICKS (so only the FOCUSED
//! window — the only one receiving routed input — sees its counter rise). No
//! allocator: everything is in static arrays.

#[link(wasm_import_module = "wm")]
extern "C" {
    fn commit(ptr: *const u8, len: u32, w: u32, h: u32);
    fn app_id() -> u32;
    fn tick();
    /// Drain ONE event into `retptr` (a 20-byte area we own). Layout:
    /// disc u32 @0 (0=none,1=some), kind @4, p0 @8, p1 @12, p2 @16 (all LE u32).
    fn poll_event(retptr: *mut u8);
}

const W: usize = 320;
const H: usize = 240;
static mut BUF: [u8; W * H * 4] = [0; W * H * 4];
static mut COUNTER: u32 = 0;
static mut CLICKS: u32 = 0;
// Scratch for one poll_event result (disc + 4×u32 = 20 bytes).
static mut EVBUF: [u8; 20] = [0; 20];

#[inline]
unsafe fn le_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[no_mangle]
pub extern "C" fn frame() {
    unsafe {
        tick();
        COUNTER = COUNTER.wrapping_add(1);

        // Drain this window's input queue: count left-button-down edges.
        let ev = core::ptr::addr_of_mut!(EVBUF) as *mut u8;
        loop {
            poll_event(ev);
            let buf = core::slice::from_raw_parts(ev as *const u8, 20);
            if le_u32(buf, 0) == 0 {
                break; // no more events
            }
            let kind = le_u32(buf, 4);
            let p0 = le_u32(buf, 8); // button (mousebtn) / scancode (key)
            let p1 = le_u32(buf, 12); // pressed
            // mousebtn (kind 2), left (p0==0), pressed (p1==1) -> a click.
            if kind == 2 && p0 == 0 && p1 == 1 {
                CLICKS = CLICKS.wrapping_add(1);
            }
        }

        // Background: cycling colour offset by this window's id.
        let id = app_id();
        let r = (COUNTER.wrapping_add(id.wrapping_mul(80)) & 0xff) as u8;
        let p = core::ptr::addr_of_mut!(BUF) as *mut u8;
        let mut i = 0;
        while i < W * H * 4 {
            *p.add(i) = r;
            *p.add(i + 1) = 0x40;
            *p.add(i + 2) = 0x80;
            *p.add(i + 3) = 0xff;
            i += 4;
        }

        // Tally bar: a white block at the left, 16px wide, whose HEIGHT grows by
        // 6px per click (capped to the surface). Encodes CLICKS visibly so the
        // screendump test can see the FOCUSED window's counter rise.
        let bar_w = 16usize;
        let bar_h = core::cmp::min((CLICKS as usize) * 6, H);
        let mut y = 0usize;
        while y < bar_h {
            let mut x = 0usize;
            while x < bar_w {
                let off = (y * W + x) * 4;
                *p.add(off) = 0xff;
                *p.add(off + 1) = 0xff;
                *p.add(off + 2) = 0xff;
                *p.add(off + 3) = 0xff;
                x += 1;
            }
            y += 1;
        }

        commit(core::ptr::addr_of!(BUF) as *const u8, (W * H * 4) as u32, W as u32, H as u32);
    }
}

#[panic_handler]
fn ph(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
```

- [ ] **Step 2: Build the guest + verify imports + precompile** (WSL). This proves the guest now imports `wm.poll_event` and still exports `frame`, then regenerates `reactor.cwasm`:
```bash
wsl -d Ubuntu -u root -e bash -lc 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && (cd tools/wt-reactor && cargo build --release --target wasm32-unknown-unknown 2>&1 | tail -6) && wasm-tools print tools/wt-reactor/target/wasm32-unknown-unknown/release/wt_reactor.wasm | grep -E "import \"wm\" \"(commit|app_id|tick|poll_event)\"|export \"frame\"" && tools/wt-precompile/target/release/wt-precompile tools/wt-reactor/target/wasm32-unknown-unknown/release/wt_reactor.wasm kernel/src/wasm/wt/reactor.cwasm 2>&1 | tail -2'
```
Expected: four `import "wm" "..."` lines (commit, app_id, tick, poll_event) + `export "frame"`; then a `wrote ...reactor.cwasm` line. If `poll_event` is absent, LTO dropped it — check the `#[link]` block names it exactly `poll_event`.

- [ ] **Step 3: Commit** (the cwasm is gitignored — only the source changes):
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add tools/wt-reactor/src/lib.rs && git commit -m "feat(wt-reactor): drain wm.poll_event + per-window click counter bar"
```

---

## Task 5: Boot-check — the spike still works after the WmState/guest changes

The boot self-test (`run_reactor_spike` → marker `reactor spike calls=5 commit_b0=0x05 pixels=307200`) exercises the SAME `WmState` + guest + `commit` path. Adding `events` and the `poll_event` import must NOT regress it (the spike never pushes events, so the guest's drain loop sees `none` immediately and CLICKS stays 0 → byte0 is still `COUNTER & 0xff` = 5 after 5 frames, because the tally bar is 0px tall and the top-left pixel byte0 is the background `r`). Confirm the existing marker still passes.

**Files:** None (verification only — the existing `run_reactor_spike` + boot marker in `interrupts.rs` are reused unchanged).

- [ ] **Step 1: Build + run the boot test** (scratch ISO — never `build/os.iso`):
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot ISO=build/cmtest.iso 2>&1 | tail -10'
```
Expected tail: `TEST_BOOT_PASS`.

- [ ] **Step 2: Assert the reactor-spike marker survived** the WmState extension:
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && grep -E "wm   reactor spike calls=5 commit_b0=0x05 pixels=307200" build/test-boot.log'
```
Expected: one matching line. byte0 = top-left pixel R channel = background `r` = `(5 + 0*80) & 0xff` = 5 (the 0px tally bar does not cover pixel (0,0) until CLICKS≥1, which never happens in the spike). If byte0 ≠ 0x05, the guest's frame path regressed — STOP and inspect the serial.

- [ ] **Step 3: Commit** (no file changes; skip if `git status` is clean. This task is a gate, not an edit.)

---

## Task 6: Visual screendump — click-to-focus moves the focus border + counter

The decisive SP2 proof: boot the compositor, screendump the idle state, send a left-click over window A (left half), screendump, send a left-click over window B (right half), screendump. The focus border must follow the click and each window's tally bar must rise only while focused.

**Files:** None (uses the existing `compositor-init.sh` launch + `build/shot.py`). This task may extend `build/shot.py` with a click-and-shot variant; if so, create a NEW helper file rather than editing `shot.py` in place (other tests depend on it).

- [ ] **Step 1: Build the compositor ISO** (scratch ISO, NOT `build/os.iso`). The init script `user-bin/compositor-init.sh` already runs `compositor`:
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso INIT_SCRIPT=user-bin/compositor-init.sh ISO=build/comptest.iso 2>&1 | tail -3'
```
Expected: ISO build `Finished` / `limine bios-install` line, no errors.

- [ ] **Step 2: Create the click-and-shot QMP helper** `build/shot_focus.py`. It connects to QMP, waits for the desktop, then for each of three phases (idle, click-A, click-B) sends mouse moves/clicks via `input-send-event` (absolute coords through QEMU's `abs` axes) and screendumps. Reuses the `/tmp/qmp.sock` + screendump pattern of `build/shot.py`.
```python
import socket, json, time, sys

sock_path = "/tmp/qmp.sock"
base = "/mnt/e/MinimalOS/BasicOperatingSystem/build"

# Connect.
for _ in range(60):
    try:
        s = socket.socket(socket.AF_UNIX); s.connect(sock_path); break
    except OSError:
        time.sleep(0.5)
else:
    print("QMP connect timeout"); sys.exit(1)

f = s.makefile("rw")
def cmd(obj):
    f.write(json.dumps(obj) + "\n"); f.flush()
    return json.loads(f.readline())

json.loads(f.readline())            # greeting
cmd({"execute": "qmp_capabilities"})

def shot(name):
    out = base + "/" + name
    r = cmd({"execute": "screendump", "arguments": {"filename": out, "format": "png"}})
    if "error" in r:
        r = cmd({"execute": "screendump", "arguments": {"filename": out[:-4] + ".ppm"}})
        print("PPM", name, r)
    else:
        print("PNG", name, r)

# QEMU absolute axis range is 0..0x7fff over the display. Map a fractional
# screen position (fx,fy in 0..1) to that range.
def abs_to(fx, fy):
    ax = int(fx * 0x7fff); ay = int(fy * 0x7fff)
    cmd({"execute": "input-send-event", "arguments": {"events": [
        {"type": "abs", "data": {"axis": "x", "value": ax}},
        {"type": "abs", "data": {"axis": "y", "value": ay}},
    ]})

def click(fx, fy):
    abs_to(fx, fy); time.sleep(0.3)
    cmd({"execute": "input-send-event", "arguments": {"events": [
        {"type": "btn", "data": {"button": "left", "down": True}}]}})
    time.sleep(0.1)
    cmd({"execute": "input-send-event", "arguments": {"events": [
        {"type": "btn", "data": {"button": "left", "down": False}}]}})
    time.sleep(0.5)

# Let the compositor boot + render.
time.sleep(14)
shot("sp2-idle.png")           # window 0 (left) focused by default

# Click LEFT window a few times (fx ~0.15 = left half, fy ~0.4).
for _ in range(4):
    click(0.15, 0.40)
shot("sp2-focus-A.png")        # left focused, left tally bar risen

# Click RIGHT window a few times (fx ~0.65 = right half, fy ~0.4).
for _ in range(4):
    click(0.65, 0.40)
shot("sp2-focus-B.png")        # right focused, right tally bar risen

cmd({"execute": "quit"})
```
Note on coordinates: the screen is `g.width` wide; window 0 = `[0, width/2)`, window 1 = `[width/2, width)`. `fx=0.15` lands in window 0; `fx=0.65` lands in window 1. `fy=0.40` is inside the 240px-tall window (top at y=0). These avoid the 2px focus border edges.

- [ ] **Step 3: Boot QEMU+KVM with QMP + run the helper.** Boot headless with a QMP unix socket and the absolute-pointer device (`usb-tablet`, so `abs` axes work), then run `build/shot_focus.py`:
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && rm -f /tmp/qmp.sock build/sp2-*.png build/sp2-*.ppm && (timeout 60 qemu-system-x86_64 -machine q35 -cpu host -enable-kvm -m 512 -no-reboot -display none -serial file:build/sp2-serial.log -qmp unix:/tmp/qmp.sock,server,nowait -device qemu-xhci -device usb-tablet -cdrom build/comptest.iso & ) && sleep 1 && python3 build/shot_focus.py 2>&1 | tail -8'
```
Expected: three `PNG sp2-*.png` lines (or PPM fallback). If QMP times out, confirm KVM is available (`-enable-kvm`); without KVM the boot is too slow for the 14s wait — raise the `time.sleep(14)` or drop `-enable-kvm` and increase the timeout to 180.

- [ ] **Step 4: Inspect the three screendumps.** Open `build/sp2-idle.png`, `build/sp2-focus-A.png`, `build/sp2-focus-B.png` (Read tool renders PNGs):
  - **idle:** two side-by-side 320×240 rectangles (left at 0,0; right at width/2,0), the LEFT one has a yellow 2px focus border (window 0 starts focused). No tally bars yet (or left only, depending on stray events).
  - **focus-A:** the LEFT window has the focus border AND a white vertical tally bar at its left edge (≈24px tall = 4 clicks × 6px). The RIGHT window has NO border and NO/short tally bar.
  - **focus-B:** the focus border + tally growth have MOVED to the RIGHT window (its bar ≈24px), the LEFT window's bar is unchanged from focus-A (it stopped receiving input once focus moved). The border is now on the right window only.
  This proves: per-window input routing, click-to-focus, the focus border tracking the active window, and that ONLY the focused window's app reacts.

- [ ] **Step 5: Send the screendumps to the user for visual confirmation** (use the SendUserFile tool with the three PNGs and a caption like "SP2 focus: idle (left focused) -> click A -> click B (focus + counter follow the click)").

- [ ] **Step 6: Commit the helper** (only if Step 2 created `build/shot_focus.py`):
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add build/shot_focus.py && git commit -m "test(wm): SP2 click-to-focus screendump helper"
```

---

## Task 7: Changelog + final review

- [ ] **Step 1:** Write `CHANGELOG/NN-26-06-05-compositor-sp2-input-focus.md` (use the NEXT free `NN` — check the highest existing number in `CHANGELOG/` first) summarising SP2: per-window event queue (`WmState.events`), `wm.poll_event` host fn, `gfx::mouse_pos()`, compositor input routing + click-to-focus + focus border, reactor guest click counter. Note it implements spec §3.4 and QEMU-verified (focus + per-window counter follow the click). List touched files: `kernel/src/gfx/mod.rs`, `kernel/src/wasm/wt/wm.rs`, `tools/wt-reactor/src/lib.rs`, `build/shot_focus.py`.

- [ ] **Step 2:** Commit the changelog:
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add CHANGELOG && git commit -m "docs(changelog): compositor SP2 input + focus routing"
```

- [ ] **Step 3:** Dispatch a final code-reviewer (superpowers:requesting-code-review) over `kernel/src/wasm/wt/wm.rs` (routing + focus + border + poll_event) and `tools/wt-reactor/src/lib.rs` (event drain + counter). Focus the review on: the borrow pattern in the routing loop (one `data_mut()` per pushed event), the 20-byte `poll_event` ABI matching between host and guest, and the `events` queue not growing unbounded (it is drained every frame by the focused app; unfocused windows accumulate nothing because the compositor only pushes to the focused one).

---

## Provides (for later sub-projects)

SP3 (window manager: drag/resize/z-order/decorations) and beyond build on these SP2 interfaces:

- **`kernel/src/gfx/mod.rs`**
  - `pub fn mouse_pos() -> (i32, i32)` — live absolute cursor position for hit-testing. SP3 uses it for title-bar grab + drag deltas.
- **`kernel/src/wasm/wt/wm.rs`**
  - `pub struct WmState { id, win_w, win_h, pixels, tick, events: VecDeque<GfxEvt> }` — the per-window store data; `events` is the per-window input queue. SP3 will add `rect`/`z`/`title` fields here (windows become movable).
  - host module `wm` now also exports `poll_event(retptr)` — drains the calling window's queue into a 20-byte `option<gfx-event>` return area (disc u32@0, kind@4, p0@8, p1@12, p2@16, all LE). Apps call this every `frame()`.
  - `fn hit(origin, px, py) -> bool` and `fn draw_border(origin, thick, color)` (private helpers) — SP3 generalises `hit`/`draw_border` to per-window `rect`s + z-order and adds title-bar decorations.
  - `pub fn run_compositor_gate(cwasm: &[u8]) -> !` — now the input-routed loop with a `focused: usize`. SP3 turns the fixed `origins` array + single `focused` index into a z-ordered, movable window list and routes drag/resize there (a click on a title bar starts a drag instead of just focusing).
- **Routing contract (assumed by SP3):** the compositor is the SOLE consumer of `crate::gfx::pop()` in compositor mode; it folds the mouse, hit-tests against window rects, and pushes window-local mouse events + pass-through key events into exactly one window's `events` queue (the focused one). SP3 extends the hit-test to z-order (topmost window under the cursor wins) and adds non-client-area (title bar) handling.
