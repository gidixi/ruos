# Compositor Sub-Projects — Interface Contract (AUTHORITATIVE)

> The four sub-project plans (`2026-06-05-compositor-sp{2,3,4,5}-*.md`) were drafted **in parallel** and drifted on the shared types/names. **This contract wins.** Where a plan's draft code uses a different struct shape, method name, or entry point than below, follow THIS document. (Diagnosed by the workflow coherence pass: 4 incompatible `Window` shapes, entry-point/`present_frame`/z-order/cursor-source mismatches.)

The gate (CHANGELOG 277, on main) currently uses `Vec<(Store<WmState>, Instance, (u32,u32))>` tuples and `run_compositor_gate(cwasm) -> !` — **no** `Window`/`Compositor` types. **SP2 introduces the canonical types below in its task body** (not just its "Provides"). SP3/SP4/SP5 EXTEND them.

## Canonical types (SP2 builds these in `kernel/src/wasm/wt/wm.rs`)

```rust
/// One window = one persistent reactor instance + its placement + decorations.
/// Surface pixels live in `store.data().pixels` (NOT a field here).
pub struct Window {
    pub id: u32,
    pub store: wasmtime::Store<WmState>,
    pub inst:  wasmtime::Instance,
    pub rect:  (u32, u32, u32, u32), // SURFACE rect (x, y, w, h), EXCLUDING decorations
    pub title: String,               // shown in the SP3 title bar; "" until SP3
    pub focused: bool,
    pub alive: bool,                 // SP5 sets false to schedule teardown
}

/// Window order in `wins` IS the z-order: index 0 = bottom, last = top.
/// There is NO `z: u32` field — `raise(idx)` moves the window to the end.
pub struct Compositor {
    pub wins: Vec<Window>,
    pub module: wasmtime::Module,        // shared AOT module; instances cheap
    pub linker: wasmtime::Linker<WmState>,
    pub focused: usize,                  // index into wins (the focused window)
    pub drag: Option<DragState>,         // SP3 adds; None until SP3
}

impl Compositor {
    pub fn new(cwasm: &[u8]) -> Compositor;        // deserialize module, build linker, 0 windows (SP5 spawns) or the gate's 2 (SP2 demo)
    pub fn run(mut self) -> !;                     // the per-frame loop (each SP extends the body)
    pub fn window_at(&self, px: i32, py: i32) -> Option<usize>; // TOPMOST window whose footprint contains (px,py); SP3 makes it decoration-aware
    pub fn set_focus(&mut self, idx: usize);       // clear old focused, set new; the ONE focus impl
    pub fn raise(&mut self, idx: usize) -> usize;  // move wins[idx] to the end (top); returns new index. Does NOT focus.
    fn frame_all(&mut self);                       // call frame() on every window's instance (the gate's get_typed_func loop, ONE copy)
    fn present(&mut self);                          // SP3 defines: composite all windows bottom->top into the back-buffer, then ONE gfx::blit. SP4 parallelizes THIS.
}
```

## Pinned decisions (resolve every coherence mismatch)

1. **Entry point stays `run_compositor_gate(cwasm) -> !`** — the executor router (`kernel/src/executor/mod.rs`) calls exactly this name; do NOT rename it. Its body becomes `Compositor::new(cwasm).run()`. No plan edits the executor.
2. **Surface pixels live in `store.data().pixels`** (the `WmState`), read via `win.store.data().pixels`. `Window` has NO `pixels` field. (SP4: do not read `w.pixels` — read `w.store.data().pixels`.)
3. **Z-order = `wins` Vec order** (0 bottom … last top). NO `Window.z` field. `raise(idx)` = move-to-end. (SP4: composite in `wins` order; do NOT `sort_by_key(|w| w.z)`.)
4. **One cursor source: `crate::gfx::mouse_pos() -> (i32, i32)`** (SP2 adds it to `kernel/src/gfx/mod.rs`). SP3/SP5 use it for hit-testing; do NOT re-track cursor from kind-1 events.
5. **One focus impl: `Compositor::set_focus`** (SP2). SP3/SP5 call it; they do NOT add their own.
6. **Per-frame compositing pipeline** (SP3 builds, SP4 parallelizes):
   - `compose_window(idx) -> (Vec<u8> footprint RGBA8888, fx, fy, fw, fh)` — the **decorated** footprint (title bar + [X] + surface) for one window. The pure-CPU unit.
   - `Compositor::present()` — composite every window's footprint bottom→top into a kernel-owned screen back-buffer (`static` RGBA buffer), then ONE `crate::gfx::blit(back_buffer, 0, 0, W, H)`. SP4 parallelizes `present` by **banding the back-buffer across the AP pool, compositing the SAME decorated footprints** (NOT raw surfaces — decorations must survive).
7. **`add_to_linker` registers the UNION of host fns**: `commit`, `app_id`, `tick` (gate) + `poll_event` (SP2) + `close` (SP5). Every guest is instantiated against this one linker. The default reactor guest (`reactor.cwasm`) imports `poll_event`, so SP4's serial-reference build must keep SP2's `poll_event` registered.
8. **SP5 uses SP3's real API**: dispatch clicks via `Compositor::window_at` + `decor::hit(win.rect, px, py) == decor::Hit::Close`; raise+focus = `raise(idx)` then `set_focus(new_idx)`. There is NO `close_button_hit`/`window_at(point)->id` of SP5's draft — use the above. On spawn call `crate::proc::register(name)`; on close/reap call `crate::proc::unregister(pid)`.
9. **Window placement invariant**: `rect.1 (sy) >= decor::TITLE_H` so the title bar is on-screen (SP3 const; SP5 honors it when placing spawned windows).

## Reading order
Implement SP2 → SP3 → SP4 → SP5. SP2 lands the canonical `Window`/`Compositor`; the other three extend the same types. When a plan's draft code conflicts with this contract, the implementer reconciles to the contract (the compiler enforces it).
