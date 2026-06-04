# ruos_gfx ABI + Framebuffer Service (Plan #4)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Give a Wasmtime GUI app the host functions it needs to own the screen:
`gfx_info` (framebuffer geometry), `gfx_blit` (copy an RGBA buffer to the
framebuffer with layout conversion), and `gfx_poll_event` (coalesced keyboard +
mouse input). Plus a kernel `gfx` service that suspends/restores the text console
while the GUI runs.

**Architecture:** A `GUI_MODE` atomic flag, when set, (a) makes the framebuffer
console skip its flush so the GUI owns the pixels, and (b) diverts keyboard
scancodes from the PTY into a GUI input queue. `gfx` reads geometry from the
existing `console::fb` globals and blits guest RGBA into the linear framebuffer,
converting to the panel's `Rgb`/`Bgr` layout. The three host fns attach to the
`Linker<WtState>` built in Plan #7.

**Depends on:** Plan #7 (Wasmtime `Linker<WtState>`, `mem` accessor), Plan #1
(mouse `pop_event`). The GUI app itself (`gui.cwasm`) is built PC-side in
`W:\Work\M\ruos-desktop`.

> **ABI CONTRACT:** the `GfxInfo`/`GfxEvent` byte layouts below MUST exactly match
> the `abi` crate in the PC repo. Treat that crate as the source of truth; if it
> differs, update this plan to match it before implementing.

**Build/run via WSL** (see memory `ruos-build-env`); verify with boot-checks +
`make iso CARGO_FEATURES=boot-checks` + QEMU `-cpu max`.

---

## File Structure

- Create: `kernel/src/gfx/mod.rs` — service: GUI_MODE, blit, info, enter/leave.
- Create: `kernel/src/wasm/wt/gfx.rs` — `ruos_gfx` host fns on the Linker.
- Modify: `kernel/src/console/fb.rs` — honor GUI_MODE in `write_str`/`tick_cursor`.
- Modify: `kernel/src/keyboard/mod.rs` — divert to GUI input queue in GUI_MODE.
- Modify: `kernel/src/main.rs` — `mod gfx;`.
- Modify: `kernel/src/wasm/wt/mod.rs` — `gfx::add_to_linker` in `run_cwasm`.

---

## Task 1: `gfx` service — geometry + blit

**Files:** Create `kernel/src/gfx/mod.rs`; modify `kernel/src/main.rs`.

- [ ] **Step 1: Declare module** — add `mod gfx;` to `kernel/src/main.rs` near `mod console;`.

- [ ] **Step 2: Implement geometry + blit + mode flag**

```rust
//! Framebuffer GUI service: lends the Limine framebuffer to a fullscreen GUI app
//! while the text console is suspended. Pixels in from the guest are RGBA8888;
//! converted to the panel layout on blit.
use core::sync::atomic::{AtomicBool, Ordering};
use crate::console::fb::{FB_VIRT, FB_PITCH, FB_BPP, PixelLayout};

pub static GUI_MODE: AtomicBool = AtomicBool::new(false);

/// Canonical app-side pixel format constant returned by gfx_info.
pub const FORMAT_RGBA8888: u32 = 1;

#[derive(Copy, Clone)]
pub struct GfxGeom { pub width: u32, pub height: u32, pub stride: u32, pub format: u32 }

/// Read framebuffer geometry from the console globals (+ the active FbInfo).
pub fn geom() -> GfxGeom {
    // width/height/pitch are published; expose them via console::fb (Task adds a getter).
    let (w, h, pitch, _bpp) = crate::console::fb::active_dims();
    GfxGeom { width: w, height: h, stride: pitch, format: FORMAT_RGBA8888 }
}

/// Blit a guest RGBA8888 rectangle (`buf`, row-major, `w*h*4` bytes) into the
/// framebuffer at (x,y), converting to the panel layout. Clips to the screen.
pub fn blit(buf: &[u8], x: u32, y: u32, w: u32, h: u32) {
    let base = FB_VIRT.load(Ordering::Acquire);
    if base.is_null() { return; }
    let pitch = FB_PITCH.load(Ordering::Acquire) as usize;
    let bpp = (FB_BPP.load(Ordering::Acquire) as usize) / 8;
    let layout = crate::console::fb::active_layout();
    let (sw, sh, _, _) = crate::console::fb::active_dims();
    for row in 0..h {
        let dy = y + row;
        if dy >= sh { break; }
        for col in 0..w {
            let dx = x + col;
            if dx >= sw { continue; }
            let si = ((row * w + col) * 4) as usize;
            if si + 3 >= buf.len() { return; }
            let (r, g, b) = (buf[si], buf[si+1], buf[si+2]);
            let off = (dy as usize) * pitch + (dx as usize) * bpp;
            // SAFETY: off is within the framebuffer (bounds-checked above).
            unsafe {
                let p = base.add(off);
                match layout {
                    PixelLayout::Rgb => { *p = r; *p.add(1) = g; *p.add(2) = b; }
                    PixelLayout::Bgr => { *p = b; *p.add(1) = g; *p.add(2) = r; }
                }
            }
        }
    }
}

/// Enter GUI mode: console stops painting; GUI owns the framebuffer.
pub fn enter() { GUI_MODE.store(true, Ordering::Release); }

/// Leave GUI mode: console repaints (caller forces a full console redraw).
pub fn leave() { GUI_MODE.store(false, Ordering::Release); crate::console::redraw_all(); }
```

- [ ] **Step 3: Add the `console::fb` getters used above**

In `kernel/src/console/fb.rs` add `pub fn active_dims() -> (u32,u32,u32,u32)` and
`pub fn active_layout() -> PixelLayout` returning the live `FbInfo` fields (store
the `FbInfo` in a global at `FramebufferConsole::new`, or expose from the existing
console singleton). Add `crate::console::redraw_all()` that re-flushes the grid.

- [ ] **Step 4: Boot-check self-test (blit then read back)**

In a boot-check, blit a 2×2 red square and verify the framebuffer pixel matches
(model on `console::fb::self_test`). Log `gfx blit self-test ok/FAIL`.

- [ ] **Step 5: Build, run, commit**

```bash
git commit -am "feat(gfx): framebuffer geometry + RGBA blit + GUI_MODE"
```

---

## Task 2: Console suspend/restore under GUI_MODE

**Files:** Modify `kernel/src/console/fb.rs`.

- [ ] **Step 1: Skip painting in GUI mode**

In `FramebufferConsole::write_str`, before `render::flush`, early-skip the blit if
`crate::gfx::GUI_MODE.load(Acquire)` is set (still feed the vte parser + serial so
logs/grid stay coherent for restore). In `tick_cursor`, return early if GUI_MODE.

- [ ] **Step 2: Build, verify text console still works with GUI_MODE=false; commit**

```bash
git commit -am "feat(console): suspend framebuffer painting in GUI mode"
```

---

## Task 3: GUI input queue + keyboard diversion

**Files:** Create input queue in `kernel/src/gfx/mod.rs`; modify `kernel/src/keyboard/mod.rs`.

- [ ] **Step 1: Define `GfxEvent` + queue (ABI must match PC `abi` crate)**

```rust
use alloc::collections::VecDeque;

/// Wire layout: kind:u32 then payload. Keep in lockstep with the PC abi crate.
/// kind 0 = key, 1 = mouse_move, 2 = mouse_btn, 3 = quit.
#[derive(Copy, Clone)]
pub struct GfxEvent { pub kind: u32, pub a: i32, pub b: i32, pub c: i32 }

static EVENTS: crate::sync::IrqMutex<VecDeque<GfxEvent>> =
    crate::sync::IrqMutex::new(VecDeque::new());

pub fn push_key(scancode: u8, pressed: bool) {
    EVENTS.lock().push_back(GfxEvent { kind: 0, a: scancode as i32, b: pressed as i32, c: 0 });
}
pub fn push_mouse_move(dx: i32, dy: i32) {
    EVENTS.lock().push_back(GfxEvent { kind: 1, a: dx, b: dy, c: 0 });
}
pub fn push_mouse_btn(left: bool, right: bool, middle: bool) {
    EVENTS.lock().push_back(GfxEvent { kind: 2, a: left as i32, b: right as i32, c: middle as i32 });
}
pub fn pop() -> Option<GfxEvent> { EVENTS.lock().pop_front() }
```

- [ ] **Step 2: Drain mouse into GUI events**

`gfx_poll_event` (Task 4) drains `crate::mouse::pop_event()` and converts to
`push_mouse_move`/`push_mouse_btn` (track button-state edges), then drains
`EVENTS`. (Mouse already has its own queue from Plan #1; GUI just consumes it.)

- [ ] **Step 3: Divert keyboard in GUI mode**

In `keyboard::keyboard_handler`, at the top after reading the scancode: if
`crate::gfx::GUI_MODE.load(Acquire)`, call `crate::gfx::push_key(scancode,
!is_release)` and `eoi(); return;` instead of pushing ASCII to the PTY. (Keep the
existing PTY path when GUI_MODE is false.)

- [ ] **Step 4: Build, commit**

```bash
git commit -am "feat(gfx): GUI input queue + keyboard diversion in GUI mode"
```

---

## Task 4: `ruos_gfx` host fns on the Wasmtime Linker

**Files:** Create `kernel/src/wasm/wt/gfx.rs`; modify `kernel/src/wasm/wt/mod.rs`.

- [ ] **Step 1: Implement the three host fns**

```rust
//! `ruos_gfx` host functions for the GUI app (Wasmtime). Memory via wt::mem.
use wasmtime::Linker;
use crate::wasm::wt::state::WtState;
use crate::wasm::wt::mem;

pub fn add_to_linker(linker: &mut Linker<WtState>) -> wasmtime::Result<()> {
    // gfx_info(out_ptr) -> 0; writes GfxInfo{w,h,stride,format} (4x u32 LE).
    linker.func_wrap("ruos_gfx", "gfx_info",
        |mut caller: wasmtime::Caller<'_, WtState>, out: i32| -> i32 {
            let g = crate::gfx::geom();
            let mut buf = [0u8; 16];
            buf[0..4].copy_from_slice(&g.width.to_le_bytes());
            buf[4..8].copy_from_slice(&g.height.to_le_bytes());
            buf[8..12].copy_from_slice(&g.stride.to_le_bytes());
            buf[12..16].copy_from_slice(&g.format.to_le_bytes());
            if mem::write(&mut caller, out as u32, &buf) { 0 } else { 28 }
        })?;

    // gfx_blit(buf_ptr, buf_len, x, y, w, h) -> 0
    linker.func_wrap("ruos_gfx", "gfx_blit",
        |mut caller: wasmtime::Caller<'_, WtState>, ptr: i32, len: i32, x: i32, y: i32, w: i32, h: i32| -> i32 {
            let bytes = match mem::read(&mut caller, ptr as u32, len as u32) { Some(b) => b, None => return 28 };
            crate::gfx::blit(&bytes, x as u32, y as u32, w as u32, h as u32);
            0
        })?;

    // gfx_poll_event(out_ptr, max, timeout_ms) -> count
    linker.func_wrap("ruos_gfx", "gfx_poll_event",
        |mut caller: wasmtime::Caller<'_, WtState>, out: i32, max: i32, _timeout_ms: i32| -> i32 {
            // Fold mouse events into the GUI queue first.
            while let Some(ev) = crate::mouse::pop_event() {
                crate::gfx::push_mouse_move(ev.dx as i32, ev.dy as i32);
                crate::gfx::push_mouse_btn(ev.left, ev.right, ev.middle);
            }
            let mut n = 0i32;
            while n < max {
                match crate::gfx::pop() {
                    Some(e) => {
                        let mut buf = [0u8; 16];
                        buf[0..4].copy_from_slice(&e.kind.to_le_bytes());
                        buf[4..8].copy_from_slice(&e.a.to_le_bytes());
                        buf[8..12].copy_from_slice(&e.b.to_le_bytes());
                        buf[12..16].copy_from_slice(&e.c.to_le_bytes());
                        let off = out as u32 + (n as u32) * 16;
                        if !mem::write(&mut caller, off, &buf) { break; }
                        n += 1;
                    }
                    None => break,
                }
            }
            // TODO (Plan #7 Task 6): when n==0, epoch-yield until next input/timeout
            // instead of returning 0 immediately (busy-poll otherwise).
            n
        })?;
    Ok(())
}
```

- [ ] **Step 2: Wire into `run_cwasm`**

In `run_cwasm` (Plan #7 Task 5), after `wasi::add_to_linker`, also call
`crate::wasm::wt::gfx::add_to_linker(&mut linker)?;`. Call `crate::gfx::enter()`
before instantiating a GUI app and `crate::gfx::leave()` after it exits. (Decide
GUI vs non-GUI by a manifest/name convention, e.g. only `gui.cwasm` enters GUI
mode — or add a `gfx_info` lazily and only `enter()` on first blit.)

- [ ] **Step 3: Build, commit**

```bash
git commit -am "feat(wt): ruos_gfx host fns (info/blit/poll_event)"
```

---

## Task 5: End-to-end smoke (kernel-side)

- [ ] **Step 1:** Add a tiny test `.cwasm` (built PC-agnostic from a wat or a
  minimal Rust wasip1 app) that calls `gfx_info` then `gfx_blit` of a solid color,
  embed via the Plan #7 pipeline, and a boot-check that runs it and verifies a
  framebuffer pixel changed. Log `gfx e2e ok/FAIL`.
- [ ] **Step 2:** Build, run under QEMU `-cpu max`, confirm; commit.

---

## Task 6: Changelog

- [ ] Create `CHANGELOG/NN-26-06-04-ruos-gfx-abi.md`; commit.

---

## Self-Review notes

- **Spec coverage:** implements spec §5 (`ruos_gfx`), §8 (console suspend/restore),
  §6 frame flow (blit + input).
- **ABI sync risk:** `GfxInfo` (4×u32) and `GfxEvent` (4×u32) layouts are the
  contract with the PC `abi` crate — verify they match before shipping.
- **Perf TODO:** `gfx_poll_event` currently returns 0 immediately when idle; real
  use needs the epoch-yield from Plan #7 Task 6 to avoid busy-spin. Dirty-rect +
  720p scaling (spec §9) are follow-ups in `blit`.
- **Verify names:** `console::fb::active_dims/active_layout/redraw_all` are NEW
  getters this plan adds; confirm the console singleton can expose them without
  breaking the existing render path.
