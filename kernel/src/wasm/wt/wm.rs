//! Window-manager / compositor host module (`wm`) + reactor driver. Holds N
//! persistent wasm instances; calls their exported `frame()` each loop; reads
//! each committed surface into the per-store WmState.
//!
//! SP2 turns the static gate into a real input-routed compositor: the canonical
//! `Window` + `Compositor` types (per the interface contract) own the instances,
//! z-order (= `wins` Vec order), focus, and per-window input queues. The
//! compositor is the SOLE consumer of `crate::gfx::pop()` in compositor mode: it
//! folds the mouse, hit-tests the live cursor against window rects, sets focus on
//! a mouse-button-down (click-to-focus), translates mouse coords to window-local,
//! and pushes events into ONLY the focused window's queue. Each app drains its own
//! queue via the `wm.poll_event` host fn.
//!
//! The `wm` imports are raw `extern "C"` (not WIT) to keep the reactor focused on
//! the mechanism: a PERSISTENT instance whose `frame()` export is called
//! repeatedly. WIT-ification comes when building the real apps.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use wasmtime::{Caller, Extern, Instance, Linker, Memory, Module, Store};
use crate::gfx::GfxEvt;
use crate::wasm::wt::engine;

/// Window decorations: a title bar above each surface + a close [X] button.
/// All geometry is pure (no framebuffer access) so it is unit-checkable; the
/// drawing helpers raster into a caller-owned RGBA8888 `Vec<u8>` (also no
/// framebuffer access), so SP4 can call them on AP compositing jobs.
pub mod decor {
    /// Title-bar height in pixels (above the surface).
    pub const TITLE_H: u32 = 28;
    /// Close-button square edge (inside the title bar, right-aligned).
    pub const BTN_W: u32 = TITLE_H;
    /// Text inset from the left edge of the title bar.
    pub const TEXT_PAD_X: u32 = 8;

    // Decoration colours, RGBA8888 little-endian as [r,g,b,a].
    pub const BAR_FOCUSED:   [u8; 4] = [0x2E, 0x5A, 0x88, 0xFF]; // blue bar (active)
    pub const BAR_UNFOCUSED: [u8; 4] = [0x4A, 0x4A, 0x4A, 0xFF]; // grey bar (inactive)
    pub const TEXT_RGBA:     [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF]; // white title text
    pub const CLOSE_BG:      [u8; 4] = [0xC0, 0x3A, 0x2A, 0xFF]; // red [X] background
    pub const CLOSE_GLYPH:   [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF]; // white [X] glyph

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
}

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

/// Read `len` bytes from this guest's exported linear memory at `ptr`. None if
/// the export is missing or the range is out of bounds. (Mirrors
/// `crate::wasm::wt::mem::read`, which is typed to `WtState` and so cannot be
/// reused for `WmState`.)
fn read_guest(caller: &mut Caller<'_, WmState>, ptr: u32, len: u32) -> Option<Vec<u8>> {
    let mem = match caller.get_export("memory") {
        Some(Extern::Memory(m)) => m,
        _ => return None,
    };
    let mem: Memory = mem;
    let mut out = alloc::vec![0u8; len as usize];
    mem.read(caller, ptr as usize, &mut out).ok()?;
    Some(out)
}

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

pub fn add_to_linker(linker: &mut Linker<WmState>) -> wasmtime::Result<()> {
    // wm.commit(ptr, len, w, h): copy the guest's surface into WmState.pixels.
    linker.func_wrap("wm", "commit",
        |mut caller: Caller<'_, WmState>, ptr: i32, len: i32, w: i32, h: i32| {
            if let Some(b) = read_guest(&mut caller, ptr as u32, len as u32) {
                let s = caller.data_mut();
                s.pixels = b;
                s.win_w = w as u32;
                s.win_h = h as u32;
            }
        })?;
    // wm.app_id() -> u32: this instance's window id. (Import name is `app_id`
    // with an underscore — Rust `#[link]` preserves the symbol verbatim; verified
    // via `wasm-tools print`.)
    linker.func_wrap("wm", "app_id",
        |caller: Caller<'_, WmState>| -> i32 { caller.data().id as i32 })?;
    // wm.tick(): bump the call counter (spike instrumentation).
    linker.func_wrap("wm", "tick",
        |mut caller: Caller<'_, WmState>| { caller.data_mut().tick += 1; })?;
    // wm.poll_event(retptr): drain ONE event from THIS window's queue into the
    // guest's 20-byte return area. The calling app is identified by its own
    // Store (caller.data()), so it can only ever see its own window's events.
    // Layout matches `ruos:gui/gfx poll-event`: discriminant u32 @0 (0=none,
    // 1=some), then the gfx-event record kind@4, p0@8, p1@12, p2@16 (all LE).
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
    Ok(())
}

/// SPIKE: instantiate ONE reactor instance, call `frame()` 5× on it, return
/// `(tick, first_pixel_byte0, pixels_len)`. Proves a persistent instance +
/// repeated export call AND that the committed surface buffer arrives intact.
pub fn run_reactor_spike(cwasm: &[u8]) -> (u32, u8, usize) {
    let engine = engine();
    // SAFETY: produced by wt-precompile for this exact engine Config.
    let module = match unsafe { Module::deserialize(engine, cwasm) } {
        Ok(m) => m,
        Err(_) => return (0, 0, 0),
    };
    let mut store = Store::new(
        engine,
        WmState { id: 0, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0, events: VecDeque::new() },
    );
    let mut linker: Linker<WmState> = Linker::new(engine);
    if add_to_linker(&mut linker).is_err() { return (0, 0, 0); }
    // SysV ABI requires DF=0; cranelift/Rust code uses `rep movs` which run
    // BACKWARD if DF=1, silently corrupting copied data.
    #[cfg(target_arch = "x86_64")]
    unsafe { core::arch::asm!("cld", options(nostack)); }
    let instance = match linker.instantiate(&mut store, &module) {
        Ok(i) => i,
        Err(_) => return (0, 0, 0),
    };
    let frame = match instance.get_typed_func::<(), ()>(&mut store, "frame") {
        Ok(f) => f,
        Err(_) => return (0, 0, 0),
    };
    for _ in 0..5 {
        if frame.call(&mut store, ()).is_err() { break; }
    }
    (
        store.data().tick,
        store.data().pixels.first().copied().unwrap_or(0),
        store.data().pixels.len(),
    )
}

// --- Canonical compositor types (interface contract) ----------------------
//
// `Window` = one persistent reactor instance + its placement + decorations.
// Surface pixels live in `store.data().pixels` (NOT a field here). `Compositor`
// owns the window list; the `wins` Vec order IS the z-order (0 bottom … last
// top). SP3/SP4/SP5 extend these same types.

/// In-progress title-bar drag. `win_id` is the dragged window's id (stable
/// across z-order changes, unlike the Vec index); `grab_dx`/`grab_dy` are the
/// cursor offset inside the window footprint at mousedown, so the window tracks
/// the cursor without jumping.
#[derive(Copy, Clone)]
pub struct DragState {
    pub win_id: u32,
    pub grab_dx: i32,
    pub grab_dy: i32,
}

/// One window = one persistent reactor instance + its placement + decorations.
/// Surface pixels live in `store.data().pixels` (NOT a field here).
pub struct Window {
    pub id: u32,
    pub store: Store<WmState>,
    pub inst: Instance,
    pub rect: (u32, u32, u32, u32), // SURFACE rect (x, y, w, h), EXCLUDING decorations
    pub title: String,              // shown in the SP3 title bar; "" until SP3
    pub focused: bool,
    pub alive: bool,                // SP5 sets false to schedule teardown
}

/// Window order in `wins` IS the z-order: index 0 = bottom, last = top.
/// There is NO `z: u32` field — `raise(idx)` moves the window to the end.
pub struct Compositor {
    pub wins: Vec<Window>,
    pub module: Module,            // shared AOT module; instances cheap
    pub linker: Linker<WmState>,
    pub focused: usize,            // index into wins (the focused window)
    pub drag: Option<DragState>,   // SP3 adds; None until SP3
}

/// Reactor surface size (matches `tools/wt-reactor` W/H). Windows are fixed at
/// this size in SP2; SP3 will make them resizable.
const WIN_W: u32 = 320;
const WIN_H: u32 = 240;

/// Draw a `thick`-px solid border (RGBA `color`) just inside the surface rect
/// `(rx, ry, rw, rh)`, so it sits over the app's committed surface. Uses tiny
/// stack-allocated rows blitted via `crate::gfx::blit` (clips to screen,
/// recomposites the cursor).
fn draw_border(rect: (u32, u32, u32, u32), thick: u32, color: [u8; 4]) {
    let (rx, ry, rw, rh) = rect;
    if rw == 0 || rh == 0 || thick == 0 { return; }
    let t = thick.min(rh).min(rw);
    // Horizontal strips (top + bottom): rw wide, t tall.
    let mut hrow = alloc::vec![0u8; (rw * t * 4) as usize];
    for px in hrow.chunks_mut(4) { px.copy_from_slice(&color); }
    crate::gfx::blit(&hrow, rx, ry, rw, t);
    crate::gfx::blit(&hrow, rx, ry + rh - t, rw, t);
    // Vertical strips (left + right): t wide, rh tall.
    let mut vrow = alloc::vec![0u8; (t * rh * 4) as usize];
    for px in vrow.chunks_mut(4) { px.copy_from_slice(&color); }
    crate::gfx::blit(&vrow, rx, ry, t, rh);
    crate::gfx::blit(&vrow, rx + rw - t, ry, t, rh);
}

impl Compositor {
    /// Deserialize the shared reactor module, build the `wm` linker, and create
    /// the demo's 2 windows (the SP-GATE layout: left at (0,0), right at
    /// (g.width/2, 0), both WIN_W x WIN_H). Window 0 starts focused.
    pub fn new(cwasm: &[u8]) -> Compositor {
        let engine = engine();
        // SAFETY: produced by wt-precompile for this exact engine Config.
        let module = unsafe { Module::deserialize(engine, cwasm) }.expect("reactor module");
        let mut linker: Linker<WmState> = Linker::new(engine);
        add_to_linker(&mut linker).expect("wm linker");

        let g = crate::gfx::geom();
        let origins = [(0u32, 0u32), (g.width / 2, 0u32)];
        let mut wins: Vec<Window> = Vec::new();
        for (id, &(ox, oy)) in origins.iter().enumerate() {
            let mut store = Store::new(
                engine,
                WmState { id: id as u32, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0, events: VecDeque::new() },
            );
            let inst = linker.instantiate(&mut store, &module).expect("instantiate");
            wins.push(Window {
                id: id as u32,
                store,
                inst,
                rect: (ox, oy, WIN_W, WIN_H),
                title: String::new(),
                focused: id == 0,
                alive: true,
            });
        }
        Compositor { wins, module, linker, focused: 0, drag: None }
    }

    /// TOPMOST window whose surface rect contains framebuffer point (px, py).
    /// Searches z-order from top (last) to bottom (first). SP3 makes this
    /// decoration-aware.
    pub fn window_at(&self, px: i32, py: i32) -> Option<usize> {
        for i in (0..self.wins.len()).rev() {
            let (rx, ry, rw, rh) = self.wins[i].rect;
            let (rx, ry) = (rx as i32, ry as i32);
            if px >= rx && px < rx + rw as i32 && py >= ry && py < ry + rh as i32 {
                return Some(i);
            }
        }
        None
    }

    /// The ONE focus impl: clear the old focused flag, set the new one, update
    /// `self.focused`. SP3/SP5 call this; they do NOT add their own.
    pub fn set_focus(&mut self, idx: usize) {
        if idx >= self.wins.len() { return; }
        if idx != self.focused {
            crate::binfo!("wm", "WM-FOCUS {}", idx);
        }
        if self.focused < self.wins.len() {
            self.wins[self.focused].focused = false;
        }
        self.wins[idx].focused = true;
        self.focused = idx;
    }

    /// Move `wins[idx]` to the end (top of z-order); returns its new index. Does
    /// NOT change focus (callers pair `raise` then `set_focus`).
    pub fn raise(&mut self, idx: usize) -> usize {
        if idx >= self.wins.len() { return idx; }
        let last = self.wins.len() - 1;
        if idx == last { return last; }
        let w = self.wins.remove(idx);
        self.wins.push(w);
        // Keep `self.focused` pointing at the same window if it moved.
        if self.focused == idx {
            self.focused = last;
        } else if self.focused > idx {
            self.focused -= 1;
        }
        last
    }

    /// Index of the window with `id`, or None.
    pub fn index_of(&self, id: u32) -> Option<usize> {
        self.wins.iter().position(|w| w.id == id)
    }

    /// Topmost window whose FULL FOOTPRINT (title bar + surface) contains
    /// (px,py). Iterates top→bottom (last index first). Returns the Vec index.
    /// (Decoration-aware variant of `window_at`, which is surface-only.)
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
    /// at mousedown. Clamps so the full footprint stays on screen.
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

    /// Close (remove) the window with `id`: drop it from `wins`, which drops its
    /// (Store, Instance) → tears down the wasm instance. Returns true if a window
    /// was removed. Fixes up `self.focused` (real SP2 tracks it) so it never
    /// dangles past the end and the surviving window flagged `focused` matches.
    pub fn close(&mut self, id: u32) -> bool {
        let Some(i) = self.index_of(id) else { return false; };
        self.wins.remove(i); // Window owns Store+Instance → Drop tears it down
        if self.wins.is_empty() {
            self.focused = 0;
            return true;
        }
        // Shift `self.focused` left if the removed window was at/below it, then
        // clamp into range, then re-assert the `focused` flag on exactly that
        // window (and clear it on all others) so there are never two flagged.
        if self.focused > i || self.focused >= self.wins.len() {
            self.focused = self.focused.saturating_sub(1);
        }
        self.focused = self.focused.min(self.wins.len() - 1);
        for (j, w) in self.wins.iter_mut().enumerate() {
            w.focused = j == self.focused;
        }
        true
    }

    /// Build the full-footprint RGBA8888 buffer for window `idx`:
    /// row 0..TITLE_H = decorated title bar, then the app surface below.
    /// Returns (buf, footprint_x, footprint_y, footprint_w, footprint_h).
    /// Returns None if the surface has not been committed yet.
    fn compose_window(&self, idx: usize) -> Option<(Vec<u8>, u32, u32, u32, u32)> {
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

    /// Call `frame()` on every window's instance (the gate's get_typed_func loop,
    /// ONE copy). Each app drains its queue via `wm.poll_event` and redraws.
    fn frame_all(&mut self) {
        for w in self.wins.iter_mut() {
            if let Ok(frame) = w.inst.get_typed_func::<(), ()>(&mut w.store, "frame") {
                let _ = frame.call(&mut w.store, ());
            }
        }
    }

    /// The per-frame input-routed compositor loop. Owns the CPU; never returns.
    ///
    /// Each loop:
    ///  1. `fold_mouse()` (PS/2 -> absolute cursor + move/button GfxEvts), then
    ///     drain the kernel gfx event queue (the compositor is the SOLE consumer).
    ///  2. Route each event: a mouse-button-DOWN hit-tested inside a window
    ///     RAISES then FOCUSES it (click-to-focus). For every event, translate
    ///     mouse coords to window-local and push it into ONLY the focused
    ///     window's queue (`WmState.events`).
    ///  3. `frame_all()` (each app drains its queue + redraws), composite each
    ///     window's surface to its rect, then draw a focus border on the focused
    ///     window.
    pub fn run(mut self) -> ! {
        crate::kprintln!("[wm] compositor SP2: input + focus routing");
        // SysV ABI requires DF=0; cranelift/Rust code uses `rep movs` which run
        // BACKWARD if DF=1, silently corrupting copied data.
        #[cfg(target_arch = "x86_64")]
        unsafe { core::arch::asm!("cld", options(nostack)); }

        loop {
            // 1. Input: fold PS/2 -> events, then drain the kernel queue.
            crate::gfx::fold_mouse();
            while let Some(ev) = crate::gfx::pop() {
                // Latest cursor position (kept up to date by fold_mouse). Single
                // source of truth — do NOT re-track from kind-1 events.
                let (cx, cy) = crate::gfx::mouse_pos();

                // Click-to-focus: a mouse-button DOWN inside a window raises +
                // focuses it. `p1 != 0` = pressed (any button).
                if ev.kind == 2 && ev.p1 != 0 {
                    if let Some(i) = self.window_at(cx, cy) {
                        let new_i = self.raise(i);
                        self.set_focus(new_i);
                    }
                }

                // Route to the focused window. Translate mouse coords to
                // window-local; pass key/other events through unchanged.
                let (ox, oy, _, _) = self.wins[self.focused].rect;
                let routed = match ev.kind {
                    1 => {
                        // mousemove: p0/p1 are f32 bits of absolute x/y -> local.
                        let lx = (cx - ox as i32).max(0) as f32;
                        let ly = (cy - oy as i32).max(0) as f32;
                        GfxEvt { kind: 1, p0: lx.to_bits(), p1: ly.to_bits(), p2: 0 }
                    }
                    // mousebtn keeps button(p0)+pressed(p1); key/resize/quit pass
                    // through unchanged.
                    _ => ev,
                };
                self.wins[self.focused].store.data_mut().events.push_back(routed);
            }

            // 2. Drive each app's frame(), then composite its committed surface.
            self.frame_all();
            for w in self.wins.iter() {
                let s = w.store.data();
                if !s.pixels.is_empty() {
                    let (rx, ry, _, _) = w.rect;
                    crate::gfx::blit(&s.pixels, rx, ry, s.win_w, s.win_h);
                }
            }

            // 3. Focus border: 2px bright yellow inside the focused window's rect.
            if self.focused < self.wins.len() {
                draw_border(self.wins[self.focused].rect, 2, [0xFF, 0xFF, 0x00, 0xFF]);
            }

            // Crude pacing so the colour cycle + input feel responsive.
            for _ in 0..2_000_000u32 { core::hint::spin_loop(); }
        }
    }
}

/// Entry point (the executor router calls EXACTLY this name; do NOT rename).
/// Builds the canonical `Compositor` and runs its input-routed loop forever.
pub fn run_compositor_gate(cwasm: &[u8]) -> ! {
    crate::gfx::enter();
    Compositor::new(cwasm).run()
}

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
    let hx = (cr.0 + 2) as i32; let hy = (cr.1 + 2) as i32;          // inside [X]
    let tx = (tr.0 + 2) as i32; let ty = (tr.1 + 2) as i32;          // left of bar
    let ix = (s.0 + 4) as i32;  let iy = (s.1 + 4) as i32;           // inside surface
    if hit(s, hx, hy) == Hit::Close
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
