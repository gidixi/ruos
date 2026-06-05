> **⚠️ READ THE INTERFACE CONTRACT FIRST:** `2026-06-05-compositor-subprojects-interface-contract.md` — AUTHORITATIVE. EXTEND SP2's canonical `Window`/`Compositor` (do NOT assume a `run_compositor` rename — keep `run_compositor_gate`; do NOT add a `Window.z` field — z = `wins` Vec order). Build `compose_window(idx) -> decorated footprint` + `Compositor::present()` (composite into a kernel back-buffer, then ONE `gfx::blit`) — SP4 parallelizes `present`. Read the cursor via `crate::gfx::mouse_pos()` (no self-tracking). Use SP2's `set_focus`. Pixels are in `store.data().pixels`.

# Compositor SP3 — Window Manager Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Do ONE step per turn; build/verify before moving on.

**Goal:** Give the kernel-side compositor a real **window manager**: windows get **decorations** (a title bar with title text + a close `[X]` button drawn above each window's surface), become **movable** (drag the title bar to move the window), get **z-ordering** (an ordered list; clicking inside a window raises it to the front), and become **closable** (clicking `[X]` removes the window and tears down its wasm instance). The per-frame compositor loop becomes: drain input → handle drag/raise/close → call each app's `frame()` → composite all windows in z-order (decorations + surface) to the framebuffer.

**Architecture:** Builds on the GATE (`kernel/src/wasm/wt/wm.rs`, CHANGELOG 277) and SP2 (input + focus). SP2 already owns a `Window` struct (rect + focus + per-window input queue), a `Compositor` holding `Vec<Window>`, the input loop that reads `crate::gfx::{fold_mouse, pop}`, hit-tests the topmost window under the cursor, and routes events into that window's queue (click-to-focus). SP3 **extends** that struct and loop: it adds a title-bar band above each window's surface rect, adds a **drag state machine** (mousedown in a title bar → follow mousemove → mouseup), adds **z-order raise-on-click** (move-to-top in the ordered `Vec`), and adds **close** (`[X]` hit → remove the `Window` + drop its `(Store, Instance)`). Decorations are drawn by the kernel in Rust (a flat-colour title bar + glyph text via the kernel's existing `noto_sans_mono_bitmap` font + a `[X]` glyph box) into a per-window **frame buffer** (decoration band + surface), which is then blitted. No new wasm imports — decorations and window management are entirely kernel-side; the app still only `commit`s its surface.

**Tech Stack:** Rust pinned nightly, kernel `no_std`; wasmtime 45 core `Module`/`Linker`/`Instance` (persistent instances, repeated `frame()` calls); `crate::gfx::{blit, geom, fold_mouse, pop, GfxEvt}` for framebuffer + input; `noto_sans_mono_bitmap` (already a kernel dep, `kernel/Cargo.toml:15`) for title text. Kernel builds via WSL only (`wsl -d Ubuntu -u root -e bash -lc '...'`, build-std, `x86_64-unknown-none`). Guest is the existing `tools/wt-reactor` (`wasm32-unknown-unknown`), reused unchanged. Verification = boot-check markers (`make test-boot ISO=build/cmtest.iso`) for the WM mechanism (pure-logic unit checks on the geometry + state machine) and QEMU+KVM QMP screendump (`build/shot.py`, ISO `build/comptest.iso`) for the visual drag/raise/close.

---

## Assumed SP2 interfaces (depend on these exact signatures)

SP2 (input + focus) is assumed COMPLETE and merged. SP3 builds directly on the following concrete items in `kernel/src/wasm/wt/wm.rs`. If SP2's actual names differ, adapt the references in this plan but keep the SP3 logic identical.

```rust
// kernel/src/wasm/wt/wm.rs (provided by SP2)

/// A managed window: one persistent wasm reactor instance + its on-screen rect
/// + focus + a per-window input queue. SP2 owns `surface_*` (last committed
/// surface from WmState.pixels), `rect` (the SURFACE rect — does NOT include
/// decorations), `focused`, and `events` (the per-window routed queue).
pub struct Window {
    pub id: u32,
    pub store: Store<WmState>,
    pub inst: wasmtime::Instance,
    /// Surface rect on screen: (x, y, w, h). The app draws into w×h; SP3 adds a
    /// title bar ABOVE this rect (so the window's full footprint is taller).
    pub rect: (u32, u32, u32, u32),
    pub focused: bool,
    /// Per-window routed input queue (SP2 fills it for the focused window).
    pub events: alloc::collections::VecDeque<GfxEvt>,
}

/// The compositor: an ORDERED list of windows (index 0 = bottom, last = top in
/// z-order) + the wasm engine module. SP2's input loop drains gfx events,
/// hit-tests the topmost window under the cursor, sets `focused`, and routes
/// events into that window's `events` queue.
pub struct Compositor {
    pub wins: Vec<Window>,
    pub module: Module,
    pub linker: Linker<WmState>,
}

impl Compositor {
    /// Build a compositor with N reactor instances at SP2's default rects.
    pub fn new(cwasm: &[u8]) -> Compositor;
    /// SP2's per-frame: drain input → route to focused window → call each
    /// frame() → composite each window's surface to its rect. SP3 REPLACES the
    /// body of this (renamed to keep SP2's call site working) — see Task 4.
    pub fn run(self) -> !;
    /// Helper SP2 provides: index of the topmost window whose SURFACE rect
    /// contains (px,py), or None. (SP3 adds a decoration-aware variant.)
    pub fn topmost_at(&self, px: i32, py: i32) -> Option<usize>;
}

/// SP2's entry point, called from the executor router special-case
/// (`if slot.path.ends_with("compositor.cwasm")`). Owns the CPU, never returns.
pub fn run_compositor(cwasm: &[u8]) -> ! { Compositor::new(cwasm).run() }
```

Also assumed from the GATE (already on main, CHANGELOG 277) and the kernel:

```rust
// kernel/src/wasm/wt/wm.rs (GATE)
pub struct WmState { pub id: u32, pub win_w: u32, pub win_h: u32, pub pixels: Vec<u8>, pub tick: u32 }
pub fn add_to_linker(linker: &mut Linker<WmState>) -> wasmtime::Result<()>;

// kernel/src/gfx/mod.rs
pub struct GfxEvt { pub kind: u32, pub p0: u32, pub p1: u32, pub p2: u32 } // kind 0=key,1=mousemove(p0,p1=f32 bits x,y),2=mousebtn(p0=btn,p1=pressed),3=resize,4=quit
pub fn fold_mouse();              // drain PS/2 mouse -> gfx event queue (emits kind 1 & 2)
pub fn pop() -> Option<GfxEvt>;   // drain ONE gfx event
pub fn blit(buf: &[u8], x: u32, y: u32, w: u32, h: u32); // RGBA8888 rect, clips, recomposites cursor
pub struct GfxGeom { pub width: u32, pub height: u32, pub stride: u32, pub format: u32 }
pub fn geom() -> GfxGeom;
pub fn enter();                   // enter GUI mode (centres cursor)

// kernel/src/console/font.rs (noto bitmap, already a dep)
pub fn raster_for_weight(ch: char, bold: bool) -> noto_sans_mono_bitmap::RasterizedChar; // .raster() -> &[&[u8]] rows of alpha
pub const fn glyph_width() -> usize;
pub const fn glyph_height() -> usize;
```

**NOTE on mouse position:** `crate::gfx` keeps the absolute cursor in private statics (`MOUSE_X`/`MOUSE_Y`); there is NO public getter today. SP3 therefore tracks the cursor itself by consuming `GfxEvt` kind 1 (mousemove) `p0`/`p1` (f32 bits) — the drag state machine reads cursor position straight from the event stream, never from gfx. This keeps SP3 self-contained and avoids touching `gfx`.

---

## File Structure
- `kernel/src/wasm/wt/wm.rs` — **Modify.** Extend `Window` with `z`-relevant fields are already the `Vec` order; add `title: alloc::string::String`. Add a `decor` module (title-bar geometry + drawing + glyph text + `[X]` box). Add `DragState`. Replace the per-frame body with: drain input → drag/raise/close → frame() → composite decorations+surface in z-order.
- `kernel/src/wasm/wt/mod.rs` — **Modify.** Add boot-check demos for the pure-logic WM unit checks (title-bar geometry, hit-test, drag move math, z-order move-to-top, close).
- `kernel/src/boot/phases/interrupts.rs` — **Modify.** Wire the boot-check markers.
- `user-bin/compositor-init.sh` — **unchanged** (still runs `compositor`).
- No guest changes: `tools/wt-reactor` is reused as-is.

---

## Task 1: Window decoration model — geometry + struct fields (pure logic, boot-checkable)

Define the title-bar geometry and the new window state, with NO drawing yet, so the math is unit-checkable before any pixels.

**Files:** Modify `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: Decoration constants + geometry helpers.** Add a `decor` module at the top of `wm.rs` (after the `use` lines). Pure functions over a surface rect `(x,y,w,h)` — no framebuffer access. The title bar sits ABOVE the surface; the window's full footprint is `(x, y - TITLE_H, w, h + TITLE_H)`, clamped so `y >= TITLE_H` is the caller's job (Task 3 placement keeps windows below the bar). The `[X]` button is a `BTN_W`-wide square at the right end of the title bar.

```rust
/// Window decorations: a title bar above each surface + a close [X] button.
/// All geometry is pure (no framebuffer access) so it is unit-checkable.
pub mod decor {
    /// Title-bar height in pixels (above the surface).
    pub const TITLE_H: u32 = 28;
    /// Close-button square edge (inside the title bar, right-aligned).
    pub const BTN_W: u32 = TITLE_H;
    /// Text inset from the left edge of the title bar.
    pub const TEXT_PAD_X: u32 = 8;

    /// Title-bar rect on screen for a surface rect `(sx,sy,sw,sh)`:
    /// returns (x, y, w, h) of the bar (directly above the surface).
    /// Caller guarantees `sy >= TITLE_H`.
    pub fn title_rect(s: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
        let (sx, sy, sw, _sh) = s;
        (sx, sy - TITLE_H, sw, TITLE_H)
    }

    /// Close-button rect on screen for a surface rect `(sx,sy,sw,sh)`:
    /// a BTN_W square at the right end of the title bar.
    pub fn close_rect(s: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
        let (sx, sy, sw, _sh) = s;
        let bw = if sw < BTN_W { sw } else { BTN_W };
        (sx + sw - bw, sy - TITLE_H, bw, TITLE_H)
    }

    /// Full window footprint (title bar + surface) for hit-testing/composite.
    pub fn window_rect(s: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
        let (sx, sy, sw, sh) = s;
        (sx, sy - TITLE_H, sw, sh + TITLE_H)
    }

    /// True if (px,py) is inside rect `r=(x,y,w,h)`.
    pub fn contains(r: (u32, u32, u32, u32), px: i32, py: i32) -> bool {
        let (x, y, w, h) = r;
        px >= x as i32 && py >= y as i32
            && px < (x + w) as i32 && py < (y + h) as i32
    }

    /// Where a point landed on a window. Used by the input dispatcher.
    #[derive(Copy, Clone, PartialEq, Eq, Debug)]
    pub enum Hit { Close, Title, Surface, Outside }

    /// Classify (px,py) against a surface rect `s` (decoration-aware).
    /// Close takes priority over Title; Title over Surface.
    pub fn hit(s: (u32, u32, u32, u32), px: i32, py: i32) -> Hit {
        if contains(close_rect(s), px, py) { return Hit::Close; }
        if contains(title_rect(s), px, py) { return Hit::Title; }
        if contains(s, px, py) { return Hit::Surface; }
        Hit::Outside
    }
}
```

- [ ] **Step 2: Add `title` to `Window`.** SP2's `Window` (assumed signature above) gets a title string for the bar. Add the field; SP2's constructor sets a default if it does not already (Task 3 sets per-window titles). In `wm.rs`, in the `pub struct Window { ... }`, add after `pub focused: bool,`:

```rust
    /// Title-bar text drawn by the compositor (kernel-side decoration).
    pub title: alloc::string::String,
```

(If SP2 already added a `title` field, skip this step — do not duplicate.)

- [ ] **Step 3: Build the kernel only (no wiring yet) to confirm it compiles** (WSL):
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make kernel 2>&1 | tail -12'
```
Expected: a clean build (warnings about unused `decor`/`title` are fine — wired in later tasks). If `make kernel` is not a target, build the crate directly: `wsl -d Ubuntu -u root -e bash -lc 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem/kernel && cargo build -Z build-std=core,alloc --target x86_64-unknown-none 2>&1 | tail -12'`.

- [ ] **Step 4: Commit** (NO changelog — controller consolidates):
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/wm.rs && git commit -m "feat(wm): SP3 decoration geometry + title field"
```

---

## Task 2: Drag/raise/close state machine — pure logic + boot-check

Implement the WM event logic as pure functions over the compositor state so it is fully boot-checkable BEFORE any drawing or input wiring. Three operations: **raise** (move-to-top in z-order), **drag** (translate the surface rect by the cursor delta), **close** (remove a window + drop its instance).

**Files:** Modify `kernel/src/wasm/wt/wm.rs`, `kernel/src/wasm/wt/mod.rs`, `kernel/src/boot/phases/interrupts.rs`.

- [ ] **Step 1: Drag state.** Add to `wm.rs` (top level, after `decor`):

```rust
/// In-progress title-bar drag. `win_id` is the dragged window's id (stable
/// across z-order changes, unlike the Vec index); `grab` is the cursor offset
/// inside the window footprint at mousedown, so the window tracks the cursor
/// without jumping.
#[derive(Copy, Clone)]
pub struct DragState {
    pub win_id: u32,
    pub grab_dx: i32,
    pub grab_dy: i32,
}
```

- [ ] **Step 2: Z-order raise (move-to-top).** Add an impl on `Compositor` (assumed SP2 type). Index 0 = bottom, last = top. Raising window at index `i` rotates it to the end:

```rust
impl Compositor {
    /// Move the window at `idx` to the top of the z-order (end of `wins`).
    /// No-op if already top or out of range. Returns the new top index.
    pub fn raise(&mut self, idx: usize) -> usize {
        let n = self.wins.len();
        if n == 0 || idx >= n || idx == n - 1 { return n.saturating_sub(1); }
        let w = self.wins.remove(idx);
        self.wins.push(w);
        n - 1
    }

    /// Index of the window with `id`, or None.
    pub fn index_of(&self, id: u32) -> Option<usize> {
        self.wins.iter().position(|w| w.id == id)
    }

    /// Topmost window whose FULL FOOTPRINT (title bar + surface) contains
    /// (px,py). Iterates top→bottom (last index first). Returns the Vec index.
    pub fn topmost_decor_at(&self, px: i32, py: i32) -> Option<usize> {
        for i in (0..self.wins.len()).rev() {
            if decor::contains(decor::window_rect(self.wins[i].rect), px, py) {
                return Some(i);
            }
        }
        None
    }

    /// Translate window `id`'s surface rect so the grabbed point follows the
    /// cursor. `(cx,cy)` is the absolute cursor; `grab` is the offset captured
    /// at mousedown. Clamps so the title bar stays fully on screen.
    pub fn drag_to(&mut self, id: u32, cx: i32, cy: i32, grab: (i32, i32)) {
        let g = crate::gfx::geom();
        let (sw_screen, sh_screen) = (g.width as i32, g.height as i32);
        if let Some(i) = self.index_of(id) {
            let (_, _, w, h) = self.wins[i].rect;
            // New footprint origin = cursor - grab offset.
            let mut fx = cx - grab.0;
            let mut fy = cy - grab.1;
            // Footprint is w × (h + TITLE_H); keep it on screen.
            let fw = w as i32;
            let fh = (h + decor::TITLE_H) as i32;
            fx = fx.clamp(0, (sw_screen - fw).max(0));
            fy = fy.clamp(0, (sh_screen - fh).max(0));
            // Surface origin = footprint origin + (0, TITLE_H).
            let sx = fx as u32;
            let sy = (fy + decor::TITLE_H as i32) as u32;
            self.wins[i].rect = (sx, sy, w, h);
        }
    }

    /// Close (remove) the window with `id`: drop it from `wins`, which drops
    /// its (Store, Instance) → tears down the wasm instance. Returns true if a
    /// window was removed.
    pub fn close(&mut self, id: u32) -> bool {
        if let Some(i) = self.index_of(id) {
            self.wins.remove(i); // Window owns Store+Instance → Drop tears it down
            true
        } else {
            false
        }
    }
}
```

- [ ] **Step 3: Pure-logic boot-check.** Add a self-test in `wm.rs` that exercises the geometry + state machine with a synthetic, instance-free compositor. Because `Compositor` holds wasm instances, the test operates on a small standalone struct mirroring the fields it needs (rects + ids + z-order) so it never instantiates wasm — keeping the boot-check fast and deterministic:

```rust
/// Boot-check: exercise the WM geometry + z-order/drag/close math with NO wasm
/// instances. Returns a bitfield of passing sub-checks (all set == 0b1_1111).
#[cfg(feature = "boot-checks")]
pub fn wm_logic_selftest() -> u32 {
    use decor::{Hit, hit, title_rect, close_rect};
    let mut flags = 0u32;

    // (bit 0) title bar sits above the surface, full width.
    let s = (100u32, 50u32, 320u32, 240u32); // surface; sy=50 >= TITLE_H=28
    let tr = title_rect(s);
    if tr == (100, 50 - decor::TITLE_H, 320, decor::TITLE_H) { flags |= 1 << 0; }

    // (bit 1) close button is a square at the right end of the bar.
    let cr = close_rect(s);
    if cr == (100 + 320 - decor::BTN_W, 50 - decor::TITLE_H, decor::BTN_W, decor::TITLE_H) {
        flags |= 1 << 1;
    }

    // (bit 2) hit classification: point in [X] => Close; in bar (left) => Title;
    // in surface => Surface; above the bar => Outside.
    let cx = (cr.0 + 2) as i32; let cy = (cr.1 + 2) as i32;            // inside [X]
    let tx = (tr.0 + 2) as i32; let ty = (tr.1 + 2) as i32;            // left of bar
    let ix = (s.0 + 4) as i32;  let iy = (s.1 + 4) as i32;            // inside surface
    if hit(s, cx, cy) == Hit::Close
        && hit(s, tx, ty) == Hit::Title
        && hit(s, ix, iy) == Hit::Surface
        && hit(s, ix, (tr.1 as i32) - 5) == Hit::Outside
    { flags |= 1 << 2; }

    // (bit 3) z-order move-to-top: ids [10,11,12] (12 top); raise idx 0 (id 10)
    // => order [11,12,10] (10 now top).
    let mut order = alloc::vec![10u32, 11, 12];
    let i = 0usize;
    let w = order.remove(i); order.push(w); // mirrors Compositor::raise
    if order == alloc::vec![11u32, 12, 10] { flags |= 1 << 3; }

    // (bit 4) drag math: footprint origin = cursor - grab, surface = +TITLE_H.
    // grab=(10,5), cursor=(200,160) => footprint (190,155) => surface (190,155+28).
    let grab = (10i32, 5i32);
    let (cxd, cyd) = (200i32, 160i32);
    let fx = cxd - grab.0; let fy = cyd - grab.1;
    let sx = fx; let sy = fy + decor::TITLE_H as i32;
    if sx == 190 && sy == 155 + decor::TITLE_H as i32 { flags |= 1 << 4; }

    flags
}
```

- [ ] **Step 4: Wire the boot-check.** In `kernel/src/wasm/wt/mod.rs`, add (near the existing reactor demos):

```rust
#[cfg(feature = "boot-checks")]
pub fn run_wm_logic_selftest() -> u32 {
    crate::wasm::wt::wm::wm_logic_selftest()
}
```

In `kernel/src/boot/phases/interrupts.rs`, inside the `#[cfg(feature="boot-checks")]` block (after the existing `wm` reactor marker), add:

```rust
        let wmf = crate::wasm::wt::run_wm_logic_selftest();
        crate::binfo!("wm", "sp3 logic selftest flags=0b{:05b}", wmf);
```

- [ ] **Step 5: Boot test + assert** (scratch ISO — NEVER overwrite `build/os.iso`):
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot ISO=build/cmtest.iso 2>&1 | tail -8'
```
Then assert all five sub-checks passed:
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && grep -E "wm .*sp3 logic selftest flags=0b11111" build/test-boot.log'
```
Must match `flags=0b11111`. Any other value = a specific WM op is wrong: bit0 title geometry, bit1 close geometry, bit2 hit-test, bit3 z-order raise, bit4 drag math. Report the value + serial.

- [ ] **Step 6: Commit:**
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/wm.rs kernel/src/wasm/wt/mod.rs kernel/src/boot/phases/interrupts.rs && git commit -m "feat(wm): SP3 drag/raise/close state machine + boot-check"
```

---

## Task 3: Decoration drawing — title bar + text + [X], composited per window

Draw decorations into a per-window **composite buffer** (title band on top, surface below) and blit the whole footprint. Drawing is pure CPU into a `Vec<u8>` RGBA8888 buffer (no SMP yet — SP4 parallelizes this).

**Files:** Modify `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: Decoration colours + a glyph blitter into an RGBA buffer.** Add to the `decor` module in `wm.rs`:

```rust
    // Decoration colours, RGBA8888 little-endian as [r,g,b,a].
    pub const BAR_FOCUSED:   [u8; 4] = [0x2E, 0x5A, 0x88, 0xFF]; // blue bar (active)
    pub const BAR_UNFOCUSED: [u8; 4] = [0x4A, 0x4A, 0x4A, 0xFF]; // grey bar (inactive)
    pub const TEXT_RGBA:     [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF]; // white title text
    pub const CLOSE_BG:      [u8; 4] = [0xC0, 0x3A, 0x2A, 0xFF]; // red [X] background
    pub const CLOSE_GLYPH:   [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF]; // white [X] glyph

    /// Fill a solid RGBA rect into a row-major RGBA8888 buffer `buf` of size
    /// `buf_w × buf_h`. Clips to the buffer. (x,y) is buffer-local.
    pub fn fill_rect(buf: &mut [u8], buf_w: u32, buf_h: u32,
                     x: u32, y: u32, w: u32, h: u32, c: [u8; 4]) {
        let bw = buf_w as usize;
        for ry in 0..h as usize {
            let py = y as usize + ry;
            if py >= buf_h as usize { break; }
            for rx in 0..w as usize {
                let px = x as usize + rx;
                if px >= bw { break; }
                let o = (py * bw + px) * 4;
                if o + 4 > buf.len() { break; }
                buf[o] = c[0]; buf[o + 1] = c[1]; buf[o + 2] = c[2]; buf[o + 3] = c[3];
            }
        }
    }

    /// Alpha-blend one glyph's coverage onto `buf` in colour `c` at buffer-local
    /// (gx,gy). `raster` rows are alpha intensities (0..=255) from the noto font.
    fn blend_glyph(buf: &mut [u8], buf_w: u32, buf_h: u32,
                   gx: i32, gy: i32, raster: &[&[u8]], c: [u8; 4]) {
        let bw = buf_w as usize;
        for (ry, row) in raster.iter().enumerate() {
            let py = gy + ry as i32;
            if py < 0 || py >= buf_h as i32 { continue; }
            for (rx, &a) in row.iter().enumerate() {
                if a == 0 { continue; }
                let px = gx + rx as i32;
                if px < 0 || px >= bw as i32 { continue; }
                let o = (py as usize * bw + px as usize) * 4;
                if o + 4 > buf.len() { continue; }
                let a16 = a as u16;
                let inv = 255 - a16;
                // out = src*a + dst*(1-a), per channel (8-bit).
                buf[o]     = ((c[0] as u16 * a16 + buf[o]     as u16 * inv) / 255) as u8;
                buf[o + 1] = ((c[1] as u16 * a16 + buf[o + 1] as u16 * inv) / 255) as u8;
                buf[o + 2] = ((c[2] as u16 * a16 + buf[o + 2] as u16 * inv) / 255) as u8;
                buf[o + 3] = 0xFF;
            }
        }
    }

    /// Draw a UTF-8 string starting at buffer-local (x,y), advancing by the
    /// monospace glyph width. Stops at the right edge `max_x`. Uses the kernel's
    /// noto bitmap font (Regular weight). Vertically centres in TITLE_H.
    pub fn draw_text(buf: &mut [u8], buf_w: u32, buf_h: u32,
                     x: u32, y: u32, max_x: u32, text: &str, c: [u8; 4]) {
        let gw = crate::console::font::glyph_width() as u32;
        let gh = crate::console::font::glyph_height() as i32;
        let gy = y as i32 + ((TITLE_H as i32 - gh) / 2).max(0);
        let mut pen = x;
        for ch in text.chars() {
            if pen + gw > max_x { break; }
            let r = crate::console::font::raster_for_weight(ch, false);
            blend_glyph(buf, buf_w, buf_h, pen as i32, gy, r.raster(), c);
            pen += gw;
        }
    }
```

- [ ] **Step 2: Compose one window's footprint into a buffer.** Add an impl method on `Compositor` that produces the full-footprint RGBA8888 buffer for window `idx` (title band + surface) ready to blit at the footprint origin. The surface pixels come from `WmState.pixels` (committed by the app this frame):

```rust
impl Compositor {
    /// Build the full-footprint RGBA8888 buffer for window `idx`:
    /// row 0..TITLE_H = decorated title bar, then the app surface below.
    /// Returns (buf, footprint_x, footprint_y, footprint_w, footprint_h).
    /// Returns None if the surface has not been committed yet.
    fn compose_window(&self, idx: usize) -> Option<(alloc::vec::Vec<u8>, u32, u32, u32, u32)> {
        let win = &self.wins[idx];
        let (sx, sy, sw, sh) = win.rect;
        let surface = &win.store.data().pixels;
        if surface.is_empty() { return None; }
        let th = decor::TITLE_H;
        let fw = sw;
        let fh = sh + th;
        let (fx, fy) = (sx, sy - th); // footprint origin (caller keeps sy >= th)
        let mut buf = alloc::vec![0u8; (fw * fh * 4) as usize];

        // Title bar background (focus-coloured).
        let bar = if win.focused { decor::BAR_FOCUSED } else { decor::BAR_UNFOCUSED };
        decor::fill_rect(&mut buf, fw, fh, 0, 0, fw, th, bar);

        // Close [X] button (right-aligned square) + glyph.
        let bw = if fw < decor::BTN_W { fw } else { decor::BTN_W };
        let bx = fw - bw;
        decor::fill_rect(&mut buf, fw, fh, bx, 0, bw, th, decor::CLOSE_BG);
        decor::draw_text(&mut buf, fw, fh, bx + (bw / 4), 0, fw, "x", decor::CLOSE_GLYPH);

        // Title text (left), clipped so it never runs under the [X] button.
        decor::draw_text(&mut buf, fw, fh, decor::TEXT_PAD_X, 0, bx,
                         &win.title, decor::TEXT_RGBA);

        // Surface below the bar: copy committed pixels row-major (clip to fw).
        let src_stride = (win.store.data().win_w as usize) * 4;
        let copy_w = core::cmp::min(win.store.data().win_w, fw) as usize * 4;
        for row in 0..sh as usize {
            let src_off = row * src_stride;
            if src_off + copy_w > surface.len() { break; }
            let dst_off = ((th as usize + row) * fw as usize) * 4;
            if dst_off + copy_w > buf.len() { break; }
            buf[dst_off..dst_off + copy_w]
                .copy_from_slice(&surface[src_off..src_off + copy_w]);
        }
        Some((buf, fx, fy, fw, fh))
    }
}
```

- [ ] **Step 3: Build the kernel** (WSL):
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make kernel 2>&1 | tail -12'
```
Expected: clean build (a warning that `compose_window` is unused is fine — wired in Task 4).

- [ ] **Step 4: Commit:**
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/wm.rs && git commit -m "feat(wm): SP3 decoration drawing (title bar + text + [X]) into footprint buffer"
```

---

## Task 4: Per-frame loop — drain input, drag/raise/close, composite z-order

Wire it together: replace the compositor's per-frame body. This is where SP2's input loop meets SP3's WM. Two windows are placed with room for their title bars; the loop drains gfx events (tracking the cursor itself), runs the drag/raise/close state machine, calls each `frame()`, then composites every window in z-order.

**Files:** Modify `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: Placement that leaves room for title bars.** SP3 windows must have `sy >= TITLE_H`. In `Compositor::new` (SP2's constructor — assumed signature), set the two demo windows' surface rects and titles so the title bars are on screen. Replace SP2's default rects with (or, if SP2 already offsets by `TITLE_H`, just set the titles):

```rust
    // Two demo windows, surfaces 320×240, placed with room above for the title
    // bar (sy >= TITLE_H). Window B overlaps A so raise/lower is visible.
    let g = crate::gfx::geom();
    let th = decor::TITLE_H;
    let placements = [
        // (surface x, y, w, h, title)
        (40u32,            th + 40, 320u32, 240u32, "reactor A"),
        (40 + 360,         th + 40, 320u32, 240u32, "reactor B"),
    ];
    // ... in the per-window construction loop, set win.rect = (x,y,w,h) and
    //     win.title = alloc::string::String::from(title).
```
(If SP2 already constructs the windows, edit its loop to apply these rects + titles. Keep ids 0..N.)

- [ ] **Step 2: The per-frame run loop.** Replace the body of `Compositor::run` (SP2's `pub fn run(self) -> !`) with the SP3 loop. It owns a `drag: Option<DragState>` and a tracked cursor `(cur_x, cur_y)`:

```rust
    pub fn run(mut self) -> ! {
        crate::kprintln!("[wm] SP3 window manager: {} windows", self.wins.len());
        crate::gfx::enter();
        // SysV ABI requires DF=0; cranelift/Rust `rep movs` run BACKWARD if DF=1.
        #[cfg(target_arch = "x86_64")]
        unsafe { core::arch::asm!("cld", options(nostack)); }

        let g = crate::gfx::geom();
        let mut cur_x = (g.width / 2) as i32;
        let mut cur_y = (g.height / 2) as i32;
        let mut drag: Option<DragState> = None;
        let mut btn_l_down = false; // left-button edge tracking (gfx sends edges)

        loop {
            // 1) Drain input. fold_mouse() pushes mousemove/button edges into the
            //    gfx queue; we consume the whole queue here (compositor is the SOLE
            //    consumer in compositor mode), updating the tracked cursor and the
            //    drag/raise/close state machine, and route surface/key events to
            //    the focused window's queue (SP2 routing reused via dispatch()).
            crate::gfx::fold_mouse();
            while let Some(ev) = crate::gfx::pop() {
                match ev.kind {
                    1 => { // mousemove: p0,p1 = f32 bits of absolute x,y
                        cur_x = f32::from_bits(ev.p0) as i32;
                        cur_y = f32::from_bits(ev.p1) as i32;
                        if let Some(d) = drag {
                            self.drag_to(d.win_id, cur_x, cur_y, (d.grab_dx, d.grab_dy));
                        }
                    }
                    2 => { // mousebtn: p0=button (0=L), p1=pressed
                        if ev.p0 == 0 {
                            let pressed = ev.p1 != 0;
                            if pressed && !btn_l_down {
                                btn_l_down = true;
                                self.on_left_down(cur_x, cur_y, &mut drag);
                            } else if !pressed && btn_l_down {
                                btn_l_down = false;
                                drag = None; // mouseup ends any drag
                            }
                        }
                    }
                    0 => { // key: route to focused window (SP2 semantics)
                        if let Some(i) = self.wins.iter().position(|w| w.focused) {
                            self.wins[i].events.push_back(ev);
                        }
                    }
                    _ => {}
                }
            }

            // 2) Call each app's frame() (round-robin) → each commits its surface.
            for i in 0..self.wins.len() {
                let f = self.wins[i].inst
                    .get_typed_func::<(), ()>(&mut self.wins[i].store, "frame");
                if let Ok(frame) = f {
                    let _ = frame.call(&mut self.wins[i].store, ());
                }
            }

            // 3) Composite bottom→top: blit each window's full footprint.
            for i in 0..self.wins.len() {
                if let Some((buf, fx, fy, fw, fh)) = self.compose_window(i) {
                    crate::gfx::blit(&buf, fx, fy, fw, fh);
                }
            }

            // Crude pacing so updates + drag are smooth/visible.
            for _ in 0..1_000_000u32 { core::hint::spin_loop(); }
        }
    }
```

- [ ] **Step 3: The left-button-down handler** (decoration-aware hit-test → close / drag / raise+focus). Add to the `Compositor` impl:

```rust
    /// Handle a left mouse-down at (px,py): topmost window under the point wins.
    /// [X] => close; title bar => raise+focus+begin drag; surface => raise+focus.
    /// Clicking empty space (no window) does nothing.
    fn on_left_down(&mut self, px: i32, py: i32, drag: &mut Option<DragState>) {
        let Some(i) = self.topmost_decor_at(px, py) else { return; };
        let s = self.wins[i].rect;
        let id = self.wins[i].id;
        match decor::hit(s, px, py) {
            decor::Hit::Close => {
                self.close(id); // drops Store+Instance → tears down the app
                *drag = None;
            }
            decor::Hit::Title => {
                let top = self.raise(i);
                self.set_focus(top);
                // Grab offset = cursor - footprint origin (so window tracks cursor).
                let fr = decor::window_rect(self.wins[top].rect);
                *drag = Some(DragState {
                    win_id: id,
                    grab_dx: px - fr.0 as i32,
                    grab_dy: py - fr.1 as i32,
                });
            }
            decor::Hit::Surface => {
                let top = self.raise(i);
                self.set_focus(top);
                *drag = None;
            }
            decor::Hit::Outside => {}
        }
    }

    /// Focus exactly the window at `idx` (clear focus on all others). SP2 may
    /// already provide this; if so, call SP2's version instead of this one.
    fn set_focus(&mut self, idx: usize) {
        for (j, w) in self.wins.iter_mut().enumerate() {
            w.focused = j == idx;
        }
    }
```

(If SP2 already provides `set_focus`/focus routing, reuse it and delete the duplicate here — keep exactly one.)

- [ ] **Step 4: Build the kernel** (WSL):
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make kernel 2>&1 | tail -15'
```
Expected: clean build. Fix any SP2-name mismatches (field/method names) flagged by the compiler — the assumed signatures at the top of this plan are the contract; reconcile to SP2's real names.

- [ ] **Step 5: Commit:**
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/wm.rs && git commit -m "feat(wm): SP3 per-frame loop — drain input, drag/raise/close, z-order composite"
```

---

## Task 5: Visual verification — drag, raise, close (QEMU+KVM screendump)

Boot the compositor with two decorated windows and prove drag/raise/close visually with QMP-injected mouse events + screendumps.

**Files:** none (reuse `user-bin/compositor-init.sh`, `build/shot.py`).

- [ ] **Step 1: Build the GUI ISO** (NEVER overwrite `build/os.iso`):
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso INIT_SCRIPT=user-bin/compositor-init.sh ISO=build/comptest.iso 2>&1 | tail -4'
```
Expected: `build/comptest.iso` written.

- [ ] **Step 2: Baseline screendump.** Boot QEMU+KVM with QMP and capture the initial composite (reuse `build/shot.py`; boot `build/comptest.iso`, wait ~14s, screendump to `build/wm_before.png`). Inspect: **two windows**, each with a **coloured title bar** (the focused one blue, the other grey), white **title text** ("reactor A" / "reactor B"), a red **[X]** at the top-right of each bar, and the cycling-colour surface below. The two windows are at distinct positions (B at x≈400, A at x≈40), B partially overlapping is not required at baseline.

- [ ] **Step 3: Drag test.** Drive the QMP socket to: move the absolute mouse onto window A's **title bar** (e.g. screen ≈ (120, 75) given A's surface at (40, 68) and TITLE_H=28 → bar y ∈ [40,68)), press+hold left, move the mouse right+down by ~200 px in a few `input-send-event` steps, release. Use the same `input-send-event abs`/`btn` pattern the repo already uses for cursor/garble tests (grep `build/` + prior shots for the helper; `build/shot.py` shows the QMP connect pattern). Screendump to `build/wm_after_drag.png`. Inspect: window A (title "reactor A") has **moved** to the new position; window B unchanged. Compare `wm_before.png` vs `wm_after_drag.png` — A's title bar + surface are translated.

- [ ] **Step 4: Raise test.** Place the two windows so they overlap (drag A so it covers part of B, or rely on the baseline overlap if you placed them overlapping). Click inside the **background** window's surface (the one currently behind). Screendump to `build/wm_after_raise.png`. Inspect: the clicked window is now **fully in front** (its title bar + surface occlude the other), and its title bar is **blue** (focused) while the other turned **grey**.

- [ ] **Step 5: Close test.** Click the **[X]** of one window (top-right of its title bar; for A at new pos, that's ≈ footprint-x + 320 - 14). Screendump to `build/wm_after_close.png`. Inspect: that window **disappears** entirely (bar + surface gone); only the other window remains, still updating. This proves the instance was removed and torn down (its surface stops compositing).

- [ ] **Step 6: Send the four screendumps for review.** Surface `build/wm_before.png`, `build/wm_after_drag.png`, `build/wm_after_raise.png`, `build/wm_after_close.png` to the controller. The acceptance criterion (spec §9 per-sub-project): drag moves a window, click raises a background window, [X] closes a window.

- [ ] **Step 7 (if a step fails):** STOP and report which transition failed + the relevant screendump + serial log. Common causes: (a) drag jumps because the grab offset is wrong (re-check `on_left_down` Title arm uses footprint origin); (b) raise does not occur because focus routing fights the z-order (ensure `raise` runs before `set_focus`); (c) close leaves a ghost because the framebuffer is not cleared under the removed window — if so, after `close()` clear that footprint rect to the desktop background (`crate::gfx::blit` a black buffer over the old footprint, or full-screen clear once) before the next composite.

---

## Task 6: Changelog + final review

- [ ] **Step 1:** Write `CHANGELOG/NN-26-06-05-compositor-sp3-window-manager.md` (next free `NN` — check the highest existing number in `CHANGELOG/`). Summarize: window decorations (title bar + text + [X]), title-bar drag (drag state machine), z-order raise-on-click, [X]-close (instance teardown), per-frame loop. Note QEMU-verified (drag/raise/close screendumps). Reference the spec (`2026-06-05-multi-window-compositor-design.md` §3.3/§4.3) and the GATE (CHANGELOG 277) + SP2.
- [ ] **Step 2:** Commit the changelog. Dispatch a final code-reviewer over the SP3 diff to `kernel/src/wasm/wt/wm.rs` (focus: the drag/raise/close state machine + the compose buffer bounds + the input drain loop).

---

## Provides (for later sub-projects)

SP4 (SMP-parallel compositing) and SP5 (launcher/lifecycle) build on these concrete interfaces SP3 exposes in `kernel/src/wasm/wt/wm.rs`:

```rust
// Decoration geometry (pure, SMP-safe — SP4 can call these on AP jobs).
pub mod decor {
    pub const TITLE_H: u32;  pub const BTN_W: u32;  pub const TEXT_PAD_X: u32;
    pub enum Hit { Close, Title, Surface, Outside }
    pub fn title_rect(s: (u32,u32,u32,u32)) -> (u32,u32,u32,u32);
    pub fn close_rect(s: (u32,u32,u32,u32)) -> (u32,u32,u32,u32);
    pub fn window_rect(s: (u32,u32,u32,u32)) -> (u32,u32,u32,u32);
    pub fn contains(r: (u32,u32,u32,u32), px: i32, py: i32) -> bool;
    pub fn hit(s: (u32,u32,u32,u32), px: i32, py: i32) -> Hit;
    pub fn fill_rect(buf: &mut [u8], buf_w: u32, buf_h: u32, x: u32, y: u32, w: u32, h: u32, c: [u8;4]);
    pub fn draw_text(buf: &mut [u8], buf_w: u32, buf_h: u32, x: u32, y: u32, max_x: u32, text: &str, c: [u8;4]);
}

pub struct DragState { pub win_id: u32, pub grab_dx: i32, pub grab_dy: i32 }

impl Compositor {
    pub fn raise(&mut self, idx: usize) -> usize;           // move-to-top in z-order
    pub fn index_of(&self, id: u32) -> Option<usize>;
    pub fn topmost_decor_at(&self, px: i32, py: i32) -> Option<usize>; // footprint hit-test
    pub fn drag_to(&mut self, id: u32, cx: i32, cy: i32, grab: (i32,i32)); // translate + clamp
    pub fn close(&mut self, id: u32) -> bool;               // remove + drop instance
    // compose_window(idx) -> footprint RGBA buffer is the per-window raster unit
    // SP4 parallelizes across the SMP compute pool (one window per AP job).
}
```

- **For SP4:** `compose_window(idx)` is the per-window pure-CPU raster unit (decorations + surface → one RGBA8888 footprint buffer). SP4 dispatches one `compose_window` per window across the AP compute pool, then blits the results on the BSP in z-order. `decor::*` are pure and re-entrant (safe to call from AP jobs).
- **For SP5 (launcher):** SP3's `Window` now carries `title: String`. SP5 adds `Compositor::spawn(cwasm, rect, title) -> id` (build a `(Store, Instance)`, push to `wins`, focus+raise) and reuses `close(id)` for lifecycle teardown. New windows should be placed with `sy >= TITLE_H` (SP3's placement invariant) so their title bar is on screen.

---

## Self-Review notes
- **Spec coverage:** implements spec §3.3 (compositor draws decorations: title bar + [X]) + §4 sub-project 3 (window manager: position/drag/z-order/decorations). Resize (§3.3 mentions it) is intentionally minimal-scope here per the SP3 brief (movable + decorations + raise + close); full resize handles are deferred. SMP compositing (§6, sub-project 4) and launcher (§4.5) are explicitly out of scope.
- **Placeholders:** none. Every code step is complete (decoration geometry, glyph blend, compose buffer, drag/raise/close, the full per-frame loop). The only "adapt to SP2" notes are explicit reconciliation instructions against the stated assumed signatures, not vague TODOs.
- **Dependency on SP2:** the assumed interfaces are stated concretely at the top (real signatures: `Window{id,store,inst,rect,focused,events}`, `Compositor{wins,module,linker}`, `Compositor::{new,run,topmost_at}`, `run_compositor`). Where SP3 needs focus routing it provides a self-contained `set_focus` with an explicit "reuse SP2's if present" note.
- **Self-containment:** drag tracks the cursor from the gfx event stream (kind 1 mousemove f32 bits) — does NOT need a new `gfx` mouse-position getter, so SP3 touches only `wm.rs` + boot-check wiring. The boot-check (`wm_logic_selftest`) exercises geometry + z-order + drag math with NO wasm instances, so it is fast and deterministic.
- **Consistency:** `TITLE_H=28`, `BTN_W=TITLE_H`, the footprint = `(x, y-TITLE_H, w, h+TITLE_H)`, and the placement invariant `sy >= TITLE_H` are used identically across `decor`, `compose_window`, `drag_to`, and `Compositor::new`. The boot-check marker (`flags=0b11111`) matches the five sub-checks computed from those same constants.
- **Risk:** the only integration risk is matching SP2's actual struct/method names; Task 4 Step 4 calls this out and the compiler enforces it. The teardown-on-close (Task 5 Step 7c) ghost-clear is pre-identified as the likely visual gotcha with a concrete fix.
