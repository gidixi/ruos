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

/// Drag state for a title-bar grab. Stub in SP2; SP3 fills it.
pub struct DragState;

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
