> **⚠️ READ THE INTERFACE CONTRACT FIRST:** `2026-06-05-compositor-subprojects-interface-contract.md` — AUTHORITATIVE. Dispatch clicks via SP3's real API: `Compositor::window_at(px,py)` + `decor::hit(win.rect, px,py) == decor::Hit::Close` + `raise(idx)` then `set_focus(new)`. There is NO `close_button_hit`/`window_at(point)->id` of this draft — use the contract's names. The shared `linker` must include `poll_event` (SP2) so a spawned `reactor.cwasm` instantiates. Call `crate::proc::register(name)` on spawn AND `crate::proc::unregister(pid)` on close. Use the canonical `Window` (pixels in `store.data().pixels`, z = Vec order, `alive` flag for teardown).

# Compositor SP5 — Launcher + Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Let the user **launch a new app as a new wasm process at runtime** from a launcher in the kernel-side compositor, and **tear it down cleanly on exit**. Concretely: the compositor draws a launcher (a taskbar strip with clickable app entries enumerating the available `.cwasm`); clicking an entry **spawns** that app as a fresh `(Store<WmState>, Instance)`, allocates a new window (rect / z-order / title), adds it to the compositor's window list, and begins calling its `frame()` in the round-robin. An app can **request close** (or be closed via the SP3 `[X]` button) → its `Store`/`Instance` is dropped, its window + surface buffer are freed, and it is removed from the round-robin. Window-ids are reused and the per-app memory budget is bounded. This is sub-project 5 of the multi-window compositor (spec `docs/superpowers/specs/2026-06-05-multi-window-compositor-design.md`, §4 item 5).

**Architecture:** The compositor already (SP2/SP3/SP4) owns a `Vec` of windows, each `{ store: Store<WmState>, inst: Instance, rect, z, title, alive }`, drives `frame()` round-robin, routes input to the focused window (click-to-focus), draws decorations + an `[X]` close button, and composites in parallel on the SMP pool. SP5 adds three things, all kernel-side Rust no_std:
1. **An app registry** — a static table of launchable apps `{ name, cwasm: &'static [u8] }`. For the test these are the embedded reactor cwasm under two display names ("react-A", "react-B"); the table is the single extension point for adding real apps later.
2. **A launcher surface** — the compositor reserves a bottom taskbar strip (drawn by the kernel in Rust, NOT a wasm app). Each registry entry is a labelled button rect. A left-click whose `(x,y)` lands inside an entry's rect calls `spawn_app(idx)`.
3. **Spawn / despawn lifecycle** — `spawn_app` deserialises the chosen module against the **shared** `Module` cache (one deserialise per distinct cwasm, N instances), creates a fresh `Store<WmState>` with a recycled window-id, instantiates, places the window at a cascading origin, and pushes it to the window list. `despawn_window` (driven by the `[X]` button from SP3, or a guest `wm.close()` request) marks the window dead; the round-robin reaps dead windows at the top of the next loop iteration, **dropping** the `Store`+`Instance` (freeing the guest linear memory) and the surface `pixels` `Vec`, and returns the window-id to a free-list for reuse.

**Tech Stack:** Rust pinned nightly; kernel wasmtime 45 **core** `Module`/`Linker`/`Instance` (persistent instances, repeated `TypedFunc<(),()>` `frame()` calls, `Store` drop = instance teardown); the existing `wm` raw host module (`commit`, `app_id`, `tick`) extended with `wm.close()`; guest `wasm32-unknown-unknown` no_std reactor (`tools/wt-reactor`, extended with a self-close variant for the test). Built via WSL `make` (`build-std`, target `x86_64-unknown-none`); guest built `wasm32-unknown-unknown`. Verification = boot-check markers (spawn/despawn mechanism, run headless) + QEMU+KVM QMP screendump + `input-send-event` clicks (visual launch + close).

---

## Assumed interfaces from prior sub-projects (SP2 → SP3 → SP4)

This plan **assumes SP2, SP3, SP4 are complete and merged.** Where SP5 depends on an interface a prior sub-project provides, the **concrete assumed signature** is stated here. If the real merged signature differs, adapt the call sites named in each task (do not silently invent a different shape).

**From SP2 (input + focus) — in `kernel/src/wasm/wt/wm.rs`:**
- A window struct extended with focus + a per-window event inbox. Assumed:
  ```rust
  pub struct Window {
      pub id: u32,
      pub store: Store<WmState>,
      pub inst: wasmtime::Instance,
      pub rect: Rect,              // (x, y, w, h) on-screen, u32
      pub z: u32,                  // higher = on top
      pub title: alloc::string::String,
      pub focused: bool,
      pub alive: bool,             // SP5 sets false to request teardown
  }
  #[derive(Copy, Clone)] pub struct Rect { pub x: u32, pub y: u32, pub w: u32, pub h: u32 }
  impl Rect { pub fn contains(&self, px: u32, py: u32) -> bool { /* x..x+w, y..y+h */ } }
  ```
- The compositor owns `struct Compositor { wins: alloc::vec::Vec<Window>, /* … */ }` and the
  main loop is a method `Compositor::run(&mut self) -> !` that, each iteration:
  (1) drains the gfx event queue via `crate::gfx::pop()` (compositor is the sole consumer),
  (2) does focus routing on mouse-button-down (click-to-focus), (3) calls each live window's
  `frame()`, (4) composites. SP5 hooks into (1) and (2) (launcher-click + close-button) and
  inserts a **reap pass** before (3).
- A helper to hit-test a screen point to a window index (top-most first):
  `fn window_at(&self, px: u32, py: u32) -> Option<usize>` (returns index into `wins`).

**From SP3 (window manager) — in `kernel/src/wasm/wt/wm.rs`:**
- Decorations: each window has a title bar + an `[X]` close button. Assumed the SP3 input
  handler exposes the close hit-test as a method that returns the **window id** clicked-to-close:
  `fn close_button_hit(&self, px: u32, py: u32) -> Option<u32>` (returns `Window.id`, None if the
  click was not on any window's `[X]`). SP5 turns that id into a teardown request via
  `request_close(id)` (added in Task 3).
- Z-order management: `fn raise(&mut self, idx: usize)` (move window `idx` to top z + focus it).
  SP5 calls this on spawn so a new window appears on top + focused.
- Decoration metrics: `const TITLEBAR_H: u32` (title-bar height in px) and
  `fn decorate(&self, w: &Window, /* compositor scratch */)` draw the frame. SP5 only needs
  `TITLEBAR_H` to size a window's surface area; it does not redraw decorations itself.

**From SP4 (SMP compositing) — in `kernel/src/wasm/wt/wm.rs`:**
- The composite step is `fn composite(&mut self)` and it iterates `self.wins` in z-order,
  blitting each live window's `store.data().pixels` to its `rect` (offloading rows/regions to
  `crate::smp::pool`). SP5 does **not** change compositing; it only changes the *set* of windows
  `composite` iterates (spawn adds, reap removes). SP5 MUST keep the invariant SP4 relies on:
  `composite` reads `store.data().pixels` which is `win_w*win_h*4` bytes — a freshly spawned
  window has empty `pixels` until its first `frame()`+`commit`, so `composite` already skips
  windows whose `pixels.is_empty()` (assumed guard, mirrors the gate's `if !s.pixels.is_empty()`).

**Unchanged, already on main (the GATE — CHANGELOG 277):**
- `WmState { id, win_w, win_h, pixels: Vec<u8>, tick }` and `add_to_linker` for host module `wm`
  (`wm.commit(ptr,len,w,h)` reads guest mem into `WmState.pixels`; `wm.app_id()->id`; `wm.tick()`).
- `crate::wasm::wt::engine() -> &'static wasmtime::Engine` (shared engine).
- `crate::gfx::{ enter, geom() -> GfxGeom{width,height,stride,format}, blit(buf,x,y,w,h),
  pop() -> Option<GfxEvt>, fold_mouse(), pending() }`; `GfxEvt{kind,p0,p1,p2}`
  (kind 0=key, 1=mousemove {p0=x f32 bits,p1=y f32 bits}, 2=mousebtn {p0=button,p1=pressed}).
- `crate::proc::{ register(name: String) -> u32, unregister(pid: u32), list() }`.
- Launch path: `/bin/compositor.cwasm` → exec router special-case in `kernel/src/executor/mod.rs`
  (`if slot.path.ends_with("compositor.cwasm") { run_compositor_gate(&bytes) }`).

---

## File Structure
- `tools/wt-reactor/src/lib.rs` — **Modify.** Add a `wm.close()` import and a second exported
  entry `frame_selfclose()` (a build-time `cfg`/feature OR a separate tiny crate) that calls
  `wm.close()` after N frames — the spawnable "closes itself" app for the despawn boot-check.
  (We add a sibling crate `tools/wt-reactor-close` to avoid perturbing the gate's `reactor.cwasm`.)
- `tools/wt-reactor-close/{Cargo.toml, src/lib.rs}` — **Create.** no_std `wasm32-unknown-unknown`
  guest: like `wt-reactor` but imports `wm.close` and calls it on frame 3; precompiled to
  `kernel/src/wasm/wt/reactor_close.cwasm`.
- `kernel/src/wasm/wt/wm.rs` — **Modify.** Add `wm.close()` host fn (sets a per-store
  `close_requested` flag), an app **registry** (`APPS: &[AppEntry]`), `Module` cache, the
  **launcher** draw + hit-test, `spawn_app`, `request_close`, the **reap pass**, and the window-id
  free-list. Extend `WmState` with `close_requested: bool`.
- `kernel/src/wasm/wt/mod.rs` — **Modify.** Embed `reactor_close.cwasm`; add a headless boot-check
  `run_lifecycle_demo() -> (u32, u32, u32)` (spawn count, peak live, final live).
- `kernel/src/boot/phases/interrupts.rs` — **Modify.** Wire the lifecycle boot-check marker.
- `kernel/src/wasm/wt/reactor_close.cwasm` — generated (gitignored) build artifact (Makefile rule).
- `Makefile` — **Modify.** Build `wt-reactor-close` + precompile → `reactor_close.cwasm`; add it to
  the `iso` / `test-boot` prerequisites and copy it to `$(ISO_ROOT)/bin/reactor-close.cwasm`.
- `build/launch_close.py` — **Create.** QMP driver: boot, screendump, click a launcher entry
  (→ 2nd window appears), screendump, click its `[X]` (→ window disappears), screendump.
- `user-bin/compositor-init.sh` — **Unchanged** (already runs `compositor`; SP5's launcher renders
  inside the same `run_compositor_gate`/`Compositor::run` path).

---

## Task 1: App registry + shared Module cache + `WmState.close_requested`

Add the data structures SP5 hangs off: a static table of launchable apps, a deserialised-`Module`
cache (one deserialise per distinct cwasm), and a per-store close flag. No behaviour change yet —
this task only compiles and is asserted by a boot-check that the registry is non-empty and a
`Module` deserialises from each entry.

**Files:** Modify `kernel/src/wasm/wt/wm.rs`, `kernel/src/wasm/wt/mod.rs`,
`kernel/src/boot/phases/interrupts.rs`.

- [ ] **Step 1: Extend `WmState`** in `kernel/src/wasm/wt/wm.rs`. Add a `close_requested` field to
  the existing struct (keep all existing fields; do NOT reorder — the gate constructs it with field
  names):
```rust
/// Per-instance store data: window id + last committed surface + close request.
pub struct WmState {
    pub id: u32,
    pub win_w: u32,
    pub win_h: u32,
    pub pixels: Vec<u8>,
    pub tick: u32,
    /// Set by the guest via `wm.close()`; the compositor reaps the window next loop.
    pub close_requested: bool,
}
```
  Then update EVERY existing `WmState { … }` literal in `wm.rs` (the gate's
  `run_reactor_spike` and `run_compositor_gate`) to add `close_requested: false`. (Grep
  `WmState {` in `wm.rs`; there are two literals in the gate code — fix both.)

- [ ] **Step 2: App registry** — at the top of `wm.rs` (after the `use` lines), add the embedded
  app blobs + the registry. The two embedded blobs are the gate's reactor (cycling colour, never
  self-closes) and the new self-closing reactor (Task 2 builds it; for now reference the file — the
  Makefile rule in Task 5 produces it before the kernel compiles):
```rust
/// A launchable app: a display name + its precompiled `.cwasm` bytes.
pub struct AppEntry {
    pub name: &'static str,
    pub cwasm: &'static [u8],
}

/// The persistent reactor (cycling colour, runs forever). Two display names so the
/// launcher shows two distinct entries that both spawn this same module.
static REACTOR_CWASM: &[u8] = include_bytes!("reactor.cwasm");
/// A reactor that calls `wm.close()` after a few frames (for the despawn boot-check
/// and the [X]-equivalent demo). Built by `tools/wt-reactor-close`.
static REACTOR_CLOSE_CWASM: &[u8] = include_bytes!("reactor_close.cwasm");

/// The launcher's app table. Adding a real app later = one more entry here.
pub static APPS: &[AppEntry] = &[
    AppEntry { name: "react-A", cwasm: REACTOR_CWASM },
    AppEntry { name: "react-B", cwasm: REACTOR_CWASM },
    AppEntry { name: "selfclose", cwasm: REACTOR_CLOSE_CWASM },
];
```
  (Note: `reactor.cwasm` is already `include_bytes!`'d under `#[cfg(feature="boot-checks")]` in
  `mod.rs`. Here in `wm.rs` it is included **unconditionally** because the compositor — not a
  boot-check — needs it. Two `include_bytes!` of the same file are fine; the linker dedups or the
  bytes are simply embedded twice, harmless.)

- [ ] **Step 3: Shared `Module` cache.** Deserialising a cwasm is expensive; the gate does it once
  per launch. For N spawns of the same app we deserialise the `Module` **once** and instantiate
  cheaply. Add a cache keyed by the cwasm pointer (each `&'static [u8]` blob has a unique address):
```rust
use spin::Mutex;
use alloc::collections::BTreeMap;

/// Cache of deserialised modules, keyed by the cwasm slice's base address (each
/// embedded blob is a distinct `&'static`, so its pointer is a stable unique key).
/// Deserialising is the costly step; instantiation off a cached `Module` is cheap.
static MODULE_CACHE: Mutex<BTreeMap<usize, Module>> = Mutex::new(BTreeMap::new());

/// Get (deserialising once, then caching) the `Module` for an app's cwasm.
fn module_for(cwasm: &'static [u8]) -> Option<Module> {
    let key = cwasm.as_ptr() as usize;
    let mut cache = MODULE_CACHE.lock();
    if let Some(m) = cache.get(&key) {
        return Some(m.clone()); // wasmtime Module is Arc-backed: clone is cheap (refcount).
    }
    // SAFETY: produced by wt-precompile for this exact engine Config.
    let m = unsafe { Module::deserialize(engine(), cwasm) }.ok()?;
    cache.insert(key, m.clone());
    Some(m)
}
```
  Add `use wasmtime::Module;` if not already imported (the gate already imports `Module` in `wm.rs`
  — keep the single import). Add `use crate::wasm::wt::engine;` if not present.

- [ ] **Step 4: Boot-check** — prove the registry + cache work without entering GUI mode. Add to
  `wm.rs`:
```rust
/// Boot self-test: every registry entry deserialises to a usable Module.
/// Returns (entry_count, modules_ok).
pub fn registry_self_test() -> (u32, u32) {
    let n = APPS.len() as u32;
    let mut ok = 0u32;
    for app in APPS {
        if module_for(app.cwasm).is_some() { ok += 1; }
    }
    (n, ok)
}
```
  In `kernel/src/wasm/wt/mod.rs`, add a thin demo wrapper:
```rust
/// Boot self-test: the launcher registry has N entries and all deserialise.
pub fn run_registry_demo() -> (u32, u32) {
    crate::wasm::wt::wm::registry_self_test()
}
```
  In `kernel/src/boot/phases/interrupts.rs`, inside the existing `#[cfg(feature="boot-checks")]`
  block (next to the reactor-spike marker), add:
```rust
        let (apps, mods_ok) = crate::wasm::wt::run_registry_demo();
        crate::binfo!("wm", "launcher registry apps={} modules_ok={}", apps, mods_ok);
```

- [ ] **Step 5: Build + assert** (scratch ISO; this task needs `reactor_close.cwasm` to exist —
  Task 2 + Task 5 create it. If you are executing strictly task-by-task, do Task 2 Steps 1–3 and
  Task 5 Step 1 FIRST so the `include_bytes!` resolves, then return here. They are independent of
  this assertion):
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot ISO=build/cmtest.iso 2>&1 | tail -8'
```
  Then assert: `grep -E "launcher registry apps=3 modules_ok=3" build/test-boot.log` must match.
  Anything else = a registry entry's cwasm failed to deserialise (wrong path, stale precompile, or
  engine-config mismatch) — report the exact `apps=`/`modules_ok=` values + serial tail.

- [ ] **Step 6: Commit** (NO changelog — controller consolidates):
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/wm.rs kernel/src/wasm/wt/mod.rs kernel/src/boot/phases/interrupts.rs && git commit -m "feat(wm): launcher app registry + shared Module cache + close_requested"
```

---

## Task 2: Self-closing reactor guest + `wm.close()` host fn

The despawn path needs an app that asks to close itself (proving the guest→kernel close request +
teardown). Add a `wm.close()` import, a sibling guest crate that calls it after 3 frames, and the
host-side flag.

**Files:** Create `tools/wt-reactor-close/{Cargo.toml, src/lib.rs}`; modify
`kernel/src/wasm/wt/wm.rs` (host fn).

- [ ] **Step 1: Guest crate** — `tools/wt-reactor-close/Cargo.toml` (mirrors `wt-reactor`):
```toml
[package]
name = "wt-reactor-close"
version = "0.0.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[profile.release]
panic = "abort"
lto = true
```

- [ ] **Step 2: Guest `tools/wt-reactor-close/src/lib.rs`** — like the gate reactor but it imports
  `wm.close` and calls it on its 3rd frame (so a spawned instance tears itself down, which the
  despawn boot-check + the visual demo both rely on):
```rust
#![no_std]

//! Self-closing reactor guest. Draws a cycling colour like `wt-reactor`, but on
//! its 3rd `frame()` it calls `wm.close()` to request its own teardown — exercises
//! the compositor despawn path (guest close request → drop Store/Instance).

#[link(wasm_import_module = "wm")]
extern "C" {
    fn commit(ptr: *const u8, len: u32, w: u32, h: u32);
    fn app_id() -> u32;
    fn tick();
    fn close();
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
        // Green-ish cycling fill so it's visually distinct from the blue reactor.
        let g = (COUNTER.wrapping_add(id.wrapping_mul(50)) & 0xff) as u8;
        let p = core::ptr::addr_of_mut!(BUF) as *mut u8;
        let mut i = 0;
        while i < W * H * 4 {
            *p.add(i) = 0x20;
            *p.add(i + 1) = g;
            *p.add(i + 2) = 0x20;
            *p.add(i + 3) = 0xff;
            i += 4;
        }
        commit(core::ptr::addr_of!(BUF) as *const u8, (W * H * 4) as u32, W as u32, H as u32);
        if COUNTER == 3 {
            close();
        }
    }
}

#[panic_handler]
fn ph(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
```

- [ ] **Step 3: Host `wm.close()`** — in `kernel/src/wasm/wt/wm.rs`, inside `add_to_linker`, after
  the existing `wm.tick` registration, add:
```rust
    // wm.close(): the guest asks the compositor to tear this window down.
    linker.func_wrap("wm", "close",
        |mut caller: Caller<'_, WmState>| { caller.data_mut().close_requested = true; })?;
```
  (`Caller` is already imported in `wm.rs` for the gate's other host fns.)

- [ ] **Step 4: Build the guest + precompile + verify imports** (WSL). This is also the artifact
  Task 1 Step 2 `include_bytes!`'s:
```bash
wsl -d Ubuntu -u root -e bash -lc 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && (cd tools/wt-reactor-close && cargo build --release --target wasm32-unknown-unknown 2>&1 | tail -6) && wasm-tools print tools/wt-reactor-close/target/wasm32-unknown-unknown/release/wt_reactor_close.wasm | grep -E "import \"wm\"|export .*frame" && tools/wt-precompile/target/release/wt-precompile tools/wt-reactor-close/target/wasm32-unknown-unknown/release/wt_reactor_close.wasm kernel/src/wasm/wt/reactor_close.cwasm 2>&1 | tail -2'
```
  Expected: imports include `wm.commit/app_id/tick/close`, exports `frame`; `wrote …reactor_close.cwasm`.

- [ ] **Step 5: Commit:**
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add tools/wt-reactor-close kernel/src/wasm/wt/wm.rs && git commit -m "feat(wm): self-closing reactor guest + wm.close() host fn"
```

---

## Task 3: Spawn / despawn lifecycle + window-id free-list (headless boot-check)

The core of SP5: instantiate a new window from a registry entry, push it to the compositor's window
list, drive its `frame()`, and reap it when it requests close — dropping the `Store`/`Instance`
(freeing guest memory) and recycling its window-id. This task proves the **mechanism headless**
(no GUI, no clicks) so it is fast and deterministic; Task 4 adds the launcher UI + clicks.

**Files:** Modify `kernel/src/wasm/wt/wm.rs`, `kernel/src/wasm/wt/mod.rs`,
`kernel/src/boot/phases/interrupts.rs`.

- [ ] **Step 1: Window-id free-list + helpers** in `wm.rs`. Window-ids must be reusable so a
  long-running session that opens/closes many windows does not exhaust the id space. Add to the
  compositor state (the SP2 `Compositor` struct already holds `wins`; add a free-list + a
  monotonic high-water mark). If you are building SP5 standalone (SP2/SP3/SP4 not literally present),
  define a minimal `Compositor` here; otherwise add these two fields to the existing struct:
```rust
    /// Window-ids returned by reaped windows, available for reuse (LIFO).
    free_ids: Vec<u32>,
    /// Next never-used id (high-water mark) — only consulted when free_ids is empty.
    next_id: u32,
```
  And the id allocator/recycler:
```rust
impl Compositor {
    /// Allocate a window-id, preferring a recycled one.
    fn alloc_id(&mut self) -> u32 {
        if let Some(id) = self.free_ids.pop() { id } else { let id = self.next_id; self.next_id += 1; id }
    }
    /// Return a reaped window's id to the free-list for reuse.
    fn free_id(&mut self, id: u32) { self.free_ids.push(id); }
}
```

- [ ] **Step 2: Memory budget guard.** Each instance = its guest linear memory + a surface buffer
  (`win_w*win_h*4` ≈ 0.3 MB for a 320×240 reactor; a 1280×800 app ≈ 4 MB). Cap the number of live
  windows so a runaway launcher can't OOM the heap. Add:
```rust
    /// Max simultaneously-live windows. Each live window holds a wasm instance
    /// (its own linear memory) + a surface buffer; this bounds the heap budget.
    /// (Reactor surface ≈ 0.3 MB; a full-window app ≈ 4 MB. 8 windows ≈ a few MB
    /// of surfaces + N linear memories — comfortably within the kernel heap.)
const MAX_WINDOWS: usize = 8;
```
  (Top-level `const` in `wm.rs`.)

- [ ] **Step 3: `spawn_app`** in `impl Compositor` — instantiate a registry entry into a new window
  and push it. Returns the new window-id, or `None` if the module failed or the budget is full:
```rust
    /// Spawn registry app `idx` as a new window. Allocates a window-id, instantiates
    /// a fresh `(Store<WmState>, Instance)` off the cached Module, places the window
    /// at a cascading origin, raises+focuses it, and pushes it to the window list.
    /// Returns the new window-id, or None (budget full / bad module / bad app idx).
    pub fn spawn_app(&mut self, idx: usize) -> Option<u32> {
        let live = self.wins.iter().filter(|w| w.alive).count();
        if live >= MAX_WINDOWS { crate::bwarn!("wm", "spawn refused: window budget full ({})", live); return None; }
        let app = APPS.get(idx)?;
        let module = module_for(app.cwasm)?;
        let id = self.alloc_id();
        let mut store = Store::new(
            engine(),
            WmState { id, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0, close_requested: false },
        );
        // SysV ABI requires DF=0 before any cranelift/Rust `rep movs`.
        #[cfg(target_arch = "x86_64")]
        unsafe { core::arch::asm!("cld", options(nostack)); }
        let inst = match self.linker.instantiate(&mut store, &module) {
            Ok(i) => i,
            Err(_) => { self.free_id(id); crate::bwarn!("wm", "spawn: instantiate failed"); return None; }
        };
        // Cascade placement: offset each new window so it doesn't fully overlap.
        let g = crate::gfx::geom();
        let n = self.wins.iter().filter(|w| w.alive).count() as u32;
        let ox = (40 + n * 28).min(g.width.saturating_sub(340));
        let oy = (40 + n * 28).min(g.height.saturating_sub(LAUNCHER_H + 260));
        let win = Window {
            id, store, inst,
            rect: Rect { x: ox, y: oy, w: 320, h: 240 },
            z: 0,
            title: alloc::string::String::from(app.name),
            focused: false,
            alive: true,
        };
        self.wins.push(win);
        let last = self.wins.len() - 1;
        self.raise(last); // SP3: move to top z + focus (assumed signature)
        let pid = crate::proc::register(alloc::format!("win:{}", app.name));
        crate::binfo!("wm", "spawn app='{}' win_id={} pid={} live={}", app.name, id, pid, live + 1);
        Some(id)
    }
```
  Notes: `self.linker` is the `Linker<WmState>` built once in `Compositor::new` (the gate builds a
  `linker` local + `add_to_linker`; SP2 should hold it on the struct — if it does not, store it on
  the struct now: `pub linker: Linker<WmState>`). `LAUNCHER_H` is defined in Task 4 Step 1 — if you
  build Task 3 before Task 4, temporarily `const LAUNCHER_H: u32 = 28;` at top of `wm.rs` and Task 4
  reuses it. `raise` is the SP3 method; if absent in your build, inline `self.wins[last].focused =
  true;` and skip z for now.

- [ ] **Step 4: `request_close` + the reap pass** in `impl Compositor`:
```rust
    /// Mark the window with this id for teardown (driven by the SP3 [X] button or a
    /// guest `wm.close()`). Idempotent; unknown ids are ignored.
    pub fn request_close(&mut self, id: u32) {
        if let Some(w) = self.wins.iter_mut().find(|w| w.id == id) {
            w.alive = false;
            crate::binfo!("wm", "close requested win_id={}", id);
        }
    }

    /// Reap dead windows: any window with `alive == false`, OR whose guest set
    /// `close_requested`. Dropping the `Window` drops its `Store` (freeing the guest
    /// linear memory) and its surface `pixels` Vec; the id returns to the free-list.
    /// Call once at the top of each compositor loop, BEFORE driving frames.
    fn reap(&mut self) {
        // Promote guest-requested closes to alive=false first.
        for w in self.wins.iter_mut() {
            if w.store.data().close_requested { w.alive = false; }
        }
        // Collect ids to recycle, then drop the dead windows.
        let mut freed: Vec<u32> = Vec::new();
        let mut i = 0;
        while i < self.wins.len() {
            if !self.wins[i].alive {
                let dead = self.wins.remove(i); // Store/Instance dropped here.
                freed.push(dead.id);
                crate::binfo!("wm", "reaped win_id={} (Store/Instance dropped)", dead.id);
            } else {
                i += 1;
            }
        }
        for id in freed { self.free_id(id); }
    }
```

- [ ] **Step 5: Wire the reap pass into the loop.** In the SP2 `Compositor::run` loop body, add a
  `self.reap();` as the FIRST statement of each iteration (before draining input and before calling
  `frame()`), so a window that requested close on its last `frame()` is gone before the next round.
  If you are building SP5 standalone, the loop skeleton is:
```rust
    loop {
        self.reap();
        // (SP2) drain input -> focus routing / launcher-click / close-button
        // (SP2/SP4) for each live window: frame() then composite
        self.frame_all();
        self.composite();
        for _ in 0..2_000_000u32 { core::hint::spin_loop(); } // pacing (gate)
    }
```
  where `frame_all` calls each live window's `frame()` (mirrors the gate's per-window
  `get_typed_func::<(),()>("frame")` + `call`).

- [ ] **Step 6: Headless lifecycle boot-check.** Add a function that exercises spawn+reap WITHOUT
  GUI mode (no `gfx::enter`, no blits, no clicks — just the instance lifecycle), so it runs in the
  fast `test-boot` path. Add to `wm.rs`:
```rust
/// Headless boot self-test of the lifecycle: build a compositor, spawn the
/// self-closing app, call frame()+reap repeatedly, and report
/// (spawns, peak_live, final_live). The self-closer requests close on frame 3,
/// so after enough rounds final_live must be 0 (instance torn down) and the id
/// recycled. Never enters GUI mode.
pub fn lifecycle_self_test() -> (u32, u32, u32) {
    let mut c = Compositor::new_headless();
    // selfclose is registry index 2 (see APPS).
    let spawns = if c.spawn_app(2).is_some() { 1 } else { 0 };
    let mut peak = 0u32;
    for _ in 0..8 {
        c.reap();
        c.frame_all();           // each live window gets one frame()
        let live = c.wins.iter().filter(|w| w.alive).count() as u32;
        if live > peak { peak = live; }
    }
    c.reap();
    let final_live = c.wins.iter().filter(|w| w.alive).count() as u32;
    // Spawn again to prove the id was recycled (free-list reuse).
    let _ = c.spawn_app(2);
    let reused = c.wins.last().map(|w| w.id).unwrap_or(u32::MAX);
    crate::binfo!("wm", "lifecycle reuse: new win_id after recycle = {}", reused);
    (spawns, peak, final_live)
}
```
  `Compositor::new_headless()` builds the struct with an empty `wins`, the `Linker<WmState>` (via
  `add_to_linker`), `free_ids: Vec::new()`, `next_id: 0`, and whatever SP2 fields default sanely —
  it must NOT call `crate::gfx::enter()`. If SP2's `Compositor::new` always enters GUI mode, add a
  `new_headless` constructor that skips the `gfx::enter()` call. `frame_all` must tolerate a window
  whose `frame()` export is missing/trapping (gate pattern: `if let Ok(f) = …get_typed_func… { let _
  = f.call(...) }`).

- [ ] **Step 7: mod.rs demo + boot marker.** In `kernel/src/wasm/wt/mod.rs`:
```rust
/// Boot self-test: spawn the self-closing app, run the loop, confirm teardown.
pub fn run_lifecycle_demo() -> (u32, u32, u32) {
    crate::wasm::wt::wm::lifecycle_self_test()
}
```
  In `kernel/src/boot/phases/interrupts.rs` (boot-checks block):
```rust
        let (sp, peak, fin) = crate::wasm::wt::run_lifecycle_demo();
        crate::binfo!("wm", "lifecycle spawns={} peak_live={} final_live={}", sp, peak, fin);
```

- [ ] **Step 8: Build + assert:**
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot ISO=build/cmtest.iso 2>&1 | tail -8'
```
  Assert: `grep -E "lifecycle spawns=1 peak_live=1 final_live=0" build/test-boot.log` must match
  (the self-closer spawned, ran, requested close on frame 3, and was reaped → 0 live), AND
  `grep -E "reaped win_id=0 \(Store/Instance dropped\)" build/test-boot.log` must match (the drop
  fired), AND `grep -E "lifecycle reuse: new win_id after recycle = 0" build/test-boot.log` must
  match (id 0 was recycled from the free-list). Any mismatch = the spawn/reap/recycle mechanism is
  broken; report the three marker lines verbatim.

- [ ] **Step 9: Commit:**
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/wm.rs kernel/src/wasm/wt/mod.rs kernel/src/boot/phases/interrupts.rs && git commit -m "feat(wm): spawn/despawn lifecycle + window-id reuse + memory budget"
```

---

## Task 4: Launcher UI (taskbar) + click-to-spawn + click-[X]-to-close (visual)

Now make it interactive: draw a launcher taskbar at the bottom of the screen with one labelled
button per registry entry; route a left-click inside an entry to `spawn_app`; route a left-click on
a window's `[X]` (SP3) — or a guest close — to `request_close`. Verified by QMP screendump + clicks.

**Files:** Modify `kernel/src/wasm/wt/wm.rs`. (Launch path + init script unchanged.)

- [ ] **Step 1: Launcher geometry + draw.** At the top of `wm.rs`:
```rust
/// Launcher (taskbar) height in px — a strip across the bottom of the screen.
const LAUNCHER_H: u32 = 28;
/// Width of each app button in the launcher.
const LAUNCHER_BTN_W: u32 = 96;
```
  Add a launcher renderer + hit-test in `impl Compositor`. The launcher is drawn by the kernel in
  Rust directly to the framebuffer via `crate::gfx::blit` (a flat-colour rect per button + a 1px
  separator). Buttons are laid out left-to-right; entry `i` occupies
  `x in [i*LAUNCHER_BTN_W, (i+1)*LAUNCHER_BTN_W)`, `y in [screen_h - LAUNCHER_H, screen_h)`:
```rust
    /// Draw the launcher taskbar across the bottom: one flat button per APP entry.
    /// Kernel-drawn (not a wasm surface). Call each loop AFTER compositing windows so
    /// the taskbar is always on top of the windows.
    fn draw_launcher(&self) {
        let g = crate::gfx::geom();
        let y0 = g.height.saturating_sub(LAUNCHER_H);
        // Dark bar background spanning the full width.
        let bar = alloc::vec![0x30u8; (g.width * LAUNCHER_H * 4) as usize];
        // Fill RGBA = (0x30,0x30,0x38,0xff): set bytes per pixel.
        let mut bar = bar; // make mutable
        for px in bar.chunks_mut(4) { px[0] = 0x30; px[1] = 0x30; px[2] = 0x38; px[3] = 0xff; }
        crate::gfx::blit(&bar, 0, y0, g.width, LAUNCHER_H);
        // Each button: a lighter rect inset by 2px, so the separators show through.
        for (i, _app) in APPS.iter().enumerate() {
            let bx = i as u32 * LAUNCHER_BTN_W;
            if bx >= g.width { break; }
            let bw = LAUNCHER_BTN_W.min(g.width - bx).saturating_sub(2);
            if bw == 0 { continue; }
            let bh = LAUNCHER_H.saturating_sub(4);
            let mut btn = alloc::vec![0u8; (bw * bh * 4) as usize];
            // Tint each button differently so they're visually distinguishable.
            let tint = 0x50u8 + (i as u8) * 0x20;
            for px in btn.chunks_mut(4) { px[0] = tint; px[1] = 0x60; px[2] = 0x90; px[3] = 0xff; }
            crate::gfx::blit(&btn, bx + 1, y0 + 2, bw, bh);
        }
    }

    /// Hit-test a screen point against the launcher. Returns the APP index if the
    /// point is inside a launcher button, else None.
    fn launcher_hit(&self, px: u32, py: u32) -> Option<usize> {
        let g = crate::gfx::geom();
        let y0 = g.height.saturating_sub(LAUNCHER_H);
        if py < y0 { return None; }
        let idx = (px / LAUNCHER_BTN_W) as usize;
        if idx < APPS.len() && (idx as u32 * LAUNCHER_BTN_W) < g.width { Some(idx) } else { None }
    }
```
  (Per-button text labels need the kernel font blitter; SP3's `decorate` already draws title text,
  so reuse its glyph helper if it is exposed. If not, the colour-tinted buttons are sufficient for
  the visual test — the click coordinates are deterministic from `LAUNCHER_BTN_W`. Drawing labels is
  a nice-to-have, NOT required for the gate.)

- [ ] **Step 2: Call `draw_launcher` in the loop.** In `Compositor::run`, after `self.composite()`
  (so the taskbar overlays the windows) and before the pacing spin, add `self.draw_launcher();`.
  (Drawing the launcher after compositing each frame keeps it on top; `crate::gfx::blit`
  recomposites the cursor over it automatically.)

- [ ] **Step 3: Route clicks.** SP2 already drains `crate::gfx::pop()` and tracks the absolute mouse
  position (the kernel folds PS/2 deltas into `MOUSE_X/MOUSE_Y` and emits `kind==1` mousemove +
  `kind==2` mousebtn events). In the SP2 input-handling code, on a left-button **press** event
  (`ev.kind == 2 && ev.p0 == 0 && ev.p1 == 1`), with the current cursor position `(mx, my)` (SP2
  tracks this; if not, read it from the last mousemove event — `f32::from_bits(ev.p0) as u32`), add
  this dispatch BEFORE the existing focus-routing, so a launcher/`[X]` click is consumed first:
```rust
        // SP5: launcher + close-button dispatch (consume the click if it hits either).
        if let Some(app_idx) = self.launcher_hit(mx, my) {
            self.spawn_app(app_idx);
        } else if let Some(close_id) = self.close_button_hit(mx, my) { // SP3 hit-test
            self.request_close(close_id);
        } else {
            // (SP2) existing click-to-focus: window_at(mx,my) -> raise(idx)
            if let Some(idx) = self.window_at(mx, my) { self.raise(idx); }
        }
```
  (`mx,my` are screen pixels. `close_button_hit` and `window_at`/`raise` are the SP3/SP2 methods
  whose assumed signatures are listed at the top of this plan. If SP2 already has the focus-routing
  block, INSERT the launcher + close branches ahead of it rather than duplicating `window_at`.)

- [ ] **Step 4: Build the GUI ISO:**
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso INIT_SCRIPT=user-bin/compositor-init.sh ISO=build/comptest.iso 2>&1 | tail -3'
```
  (Never overwrite `build/os.iso`.) Expect `make` to finish with the limine bios-install line.

- [ ] **Step 5: QMP driver `build/launch_close.py`** — boot, screendump the initial desktop (1
  window + launcher), click launcher entry 0 (spawns a 2nd window), screendump, then click that
  window's `[X]` (it disappears), screendump. The launcher button 0 center is at
  `(LAUNCHER_BTN_W/2, screen_h - LAUNCHER_H/2)` = `(48, height-14)`. The cursor starts centred
  (`gfx::enter` centres it at `w/2,h/2`); we move relative toward the target. Compute the screen
  size from the known QEMU mode (the gate boots 1280×800 by default — verify with the first
  screendump). Create `build/launch_close.py`:
```python
import socket, json, time, sys

SOCK = "/tmp/qmp.sock"
OUT = "/mnt/e/MinimalOS/BasicOperatingSystem/build"

for _ in range(60):
    try:
        s = socket.socket(socket.AF_UNIX); s.connect(SOCK); break
    except OSError:
        time.sleep(0.5)
else:
    print("QMP timeout"); sys.exit(1)

f = s.makefile("rw")
def cmd(o):
    f.write(json.dumps(o) + "\n"); f.flush(); return json.loads(f.readline())

json.loads(f.readline())                      # greeting
cmd({"execute": "qmp_capabilities"})

def move_rel(dx, dy, steps=12):
    sx = dx / steps; sy = dy / steps
    for _ in range(steps):
        cmd({"execute": "input-send-event", "arguments": {"events": [
            {"type": "rel", "data": {"axis": "x", "value": int(sx)}},
            {"type": "rel", "data": {"axis": "y", "value": int(sy)}}]}})
        time.sleep(0.05)

def click():
    cmd({"execute": "input-send-event", "arguments": {"events": [
        {"type": "btn", "data": {"button": "left", "down": True}}]}})
    time.sleep(0.12)
    cmd({"execute": "input-send-event", "arguments": {"events": [
        {"type": "btn", "data": {"button": "left", "down": False}}]}})

def shot(name):
    cmd({"execute": "screendump", "arguments": {"filename": f"{OUT}/{name}", "format": "png"}})

time.sleep(16)                                # boot + first render
shot("launch-0-initial.png")                  # 1 window + launcher taskbar

# Cursor starts centred (~640,400 on 1280x800). Launcher button 0 center ≈ (48, 786).
# Move down-left toward it: dx ≈ 48-640 = -592, dy ≈ 786-400 = +386.
move_rel(-592, 386)
time.sleep(0.4)
click()                                       # spawn app 0 -> 2nd window appears
time.sleep(1.5)
shot("launch-1-spawned.png")                  # 2 windows now

# Click the 2nd (top-most, focused) window's [X]. SP3 draws [X] at the window's
# top-right; the spawned window's rect is the cascade origin (≈40,40) size 320x240,
# so its [X] is near (40+320-12, 40+6) ≈ (348,46). Move from launcher (48,786) to there:
# dx ≈ 348-48 = +300, dy ≈ 46-786 = -740.
move_rel(300, -740)
time.sleep(0.4)
click()                                       # request close -> window reaped
time.sleep(1.0)
shot("launch-2-closed.png")                   # back to 1 window

cmd({"execute": "quit"})
print("done")
```
  (If the first screendump shows a different resolution, recompute the launcher/`[X]` targets from
  the actual width/height — they are pure arithmetic from `LAUNCHER_BTN_W`, `LAUNCHER_H`, and the
  cascade origin in `spawn_app`.)

- [ ] **Step 6: Boot QEMU+KVM with QMP + run the driver:**
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && rm -f /tmp/qmp.sock && (timeout 40 qemu-system-x86_64 -enable-kvm -cpu host -machine q35 -m 512 -no-reboot -display none -serial file:build/comp-serial.log -qmp unix:/tmp/qmp.sock,server,nowait -device qemu-xhci -cdrom build/comptest.iso & sleep 1 && python3 build/launch_close.py) 2>&1 | tail -10'
```
  (Mirrors the `click_off.py` invocation pattern; `-enable-kvm -cpu host` for speed, headless
  `-display none`, QMP on `/tmp/qmp.sock`, serial to a log so the spawn/reap `binfo!` markers are
  captured.)

- [ ] **Step 7: Inspect the three screendumps + the serial log.**
  - `build/launch-0-initial.png`: ONE window (the boot window) + the launcher taskbar strip across
    the bottom with the tinted app buttons.
  - `build/launch-1-spawned.png`: TWO windows — the original + a freshly spawned one (its surface
    a distinct colour). Proves click-to-spawn instantiated a new `(Store,Instance)` and added it to
    the round-robin (it is drawing).
  - `build/launch-2-closed.png`: back to ONE window — the spawned window is gone. Proves the `[X]`
    click → `request_close` → reap dropped the instance + freed the window.
  - In `build/comp-serial.log`: `grep -E "spawn app=.* win_id=.* live=2" build/comp-serial.log`
    (the spawn fired and brought live to 2) and `grep -E "reaped win_id=.* \(Store/Instance
    dropped\)" build/comp-serial.log` (the teardown fired). Both must match. If the spawned window
    never appears, check the serial log for the `spawn` line (click missed the button → recompute
    targets from the first screendump's actual resolution).

- [ ] **Step 8: Commit:**
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/wm.rs build/launch_close.py && git commit -m "feat(wm): launcher taskbar + click-to-spawn + click-X-to-close (visual)"
```

---

## Task 5: Makefile wiring (build + ship the self-closing reactor)

**Files:** Modify `Makefile`.

- [ ] **Step 1: Build rule for `reactor_close.cwasm`** — mirror the `reactor.cwasm` rule (it lives
  near line 151). Add right after it:
```makefile
# Self-closing reactor guest (SP5 lifecycle demo): wasm32-unknown-unknown, no_std,
# precompiled to a CORE .cwasm (not a component). Imports wm.close; calls it on frame 3.
kernel/src/wasm/wt/reactor_close.cwasm: tools/wt-reactor-close/src/lib.rs tools/wt-reactor-close/Cargo.toml $(WT_PRECOMPILE)
	@mkdir -p build
	source $$HOME/.cargo/env && cd tools/wt-reactor-close && \
		cargo build --release --target wasm32-unknown-unknown
	$(WT_PRECOMPILE) tools/wt-reactor-close/target/wasm32-unknown-unknown/release/wt_reactor_close.wasm kernel/src/wasm/wt/reactor_close.cwasm
```

- [ ] **Step 2: Add it as a prerequisite + ship it.** In BOTH the `iso:` target (line ~157) and the
  `test-boot:` target (line ~377), add `kernel/src/wasm/wt/reactor_close.cwasm` to the prerequisite
  list (next to `kernel/src/wasm/wt/reactor.cwasm`). And in BOTH the `iso:` recipe and the
  `test-boot:` recipe, next to the existing
  `cp kernel/src/wasm/wt/reactor.cwasm $(ISO_ROOT)/bin/compositor.cwasm` line, add:
```makefile
	cp kernel/src/wasm/wt/reactor_close.cwasm $(ISO_ROOT)/bin/reactor-close.cwasm
```
  (The kernel embeds it via `include_bytes!`, so shipping it to `/bin` is not strictly required for
  the launcher — the registry uses the embedded bytes — but it keeps `/bin` and the embedded set in
  sync and lets a future on-disk registry pick it up.)

- [ ] **Step 3: Add `kernel/src/wasm/wt/reactor_close.cwasm` to `.gitignore`** (it is a generated
  artifact, like `reactor.cwasm`). Check the existing ignore entry for `reactor.cwasm` and add a
  sibling line:
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && grep -n "reactor.cwasm" .gitignore || echo "NO reactor.cwasm ignore entry — check how reactor.cwasm is ignored"'
```
  If `reactor.cwasm` is ignored by an explicit line, add `kernel/src/wasm/wt/reactor_close.cwasm`
  beside it (use the Edit tool on `.gitignore`). If it is ignored by a `*.cwasm` glob, nothing to do.

- [ ] **Step 4: Clean rebuild to confirm the Make graph:**
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && rm -f kernel/src/wasm/wt/reactor_close.cwasm && make kernel/src/wasm/wt/reactor_close.cwasm 2>&1 | tail -4 && ls -l kernel/src/wasm/wt/reactor_close.cwasm'
```
  Expect the cwasm to be (re)built and listed. Then re-run the Task 3 `test-boot` assertion to
  confirm the full build still passes end-to-end:
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot ISO=build/cmtest.iso 2>&1 | tail -6 && grep -E "lifecycle spawns=1 peak_live=1 final_live=0|launcher registry apps=3 modules_ok=3" build/test-boot.log'
```

- [ ] **Step 5: Commit:**
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add Makefile .gitignore && git commit -m "build(wm): build + ship the self-closing reactor (SP5 lifecycle)"
```

---

## Task 6: Changelog + final review

- [ ] **Step 1:** Write `CHANGELOG/NN-26-06-05-compositor-sp5-launcher-lifecycle.md` (next free
  `NN`; check the highest existing number in `CHANGELOG/` first) summarising SP5: launcher taskbar
  + app registry + shared Module cache; runtime spawn of a new `(Store<WmState>, Instance)` as a new
  window in the round-robin; lifecycle teardown (guest `wm.close()` or SP3 `[X]` → reap → drop
  Store/Instance → free surface + recycle window-id); memory budget (`MAX_WINDOWS`). Note the boot
  marker `lifecycle spawns=1 peak_live=1 final_live=0` + the QEMU screendump proof (1 window → click
  launcher → 2 windows → click `[X]` → 1 window). Use the project changelog format (Cosa / Perché /
  File toccati).

- [ ] **Step 2:** Commit the changelog. Dispatch a final code-reviewer over `kernel/src/wasm/wt/wm.rs`
  (the SP5 additions: registry, cache, `spawn_app`, `request_close`, `reap`, free-list, launcher)
  + the `tools/wt-reactor-close` guest.

---

## Provides (for later sub-projects)

SP5 exposes, in `kernel/src/wasm/wt/wm.rs`:
- `pub struct AppEntry { pub name: &'static str, pub cwasm: &'static [u8] }` and
  `pub static APPS: &[AppEntry]` — the launcher's app table. **Adding a real app = one entry** here
  (a future on-disk registry can replace the static slice with a `/bin`-scanned `Vec<AppEntry>`).
- `impl Compositor { pub fn spawn_app(&mut self, idx: usize) -> Option<u32> }` — instantiate
  registry app `idx` as a new window, returning its window-id (None on budget-full / bad module).
  A future "open file with app" or IPC-launch path calls this.
- `impl Compositor { pub fn request_close(&mut self, id: u32) }` — request teardown of window `id`
  (idempotent). The reap pass (top of the loop) drops the `Store`/`Instance`, frees the surface, and
  recycles the id.
- `WmState.close_requested: bool` + the `wm.close()` host import — the guest→compositor close
  protocol. Any future app calls `wm.close()` to exit.
- `const MAX_WINDOWS: usize` — the live-window budget; later sub-projects (e.g. a memory-pressure
  policy) tune or replace it.

## Self-Review notes
- **Spec coverage:** implements spec §4 item 5 (launcher/lifecycle: spawn an app as a process +
  placement + cleanup) on top of SP2 (input/focus), SP3 (window manager), SP4 (SMP compositing).
  The launcher is the §3.3 compositor drawing a kernel-Rust UI element; spawn reuses the §3.1
  reactor model + §3.2 surface commit; teardown is the lifecycle the spec leaves to this
  sub-project.
- **Assumed interfaces stated:** SP2 (`Compositor{wins}`, `Window{id,store,inst,rect,z,title,
  focused,alive}`, `Rect::contains`, `window_at`, `run` loop draining `gfx::pop`), SP3
  (`close_button_hit -> Option<u32>`, `raise(idx)`, `TITLEBAR_H`, decorations), SP4
  (`composite` iterating live windows, skipping empty `pixels`). Each is a concrete signature, and
  each call site says what to do if the real merged signature differs.
- **Placeholders:** none. The guest (`wt-reactor-close`), every host fn (`wm.close`), `spawn_app`,
  `request_close`, `reap`, the free-list, the launcher draw/hit-test, and all three boot markers are
  complete code with exact strings. The headless boot-check (Task 3) makes the despawn assertion
  deterministic without relying on click timing.
- **Risks:** (1) the spawned window's `[X]` screen coordinates depend on SP3's decoration layout +
  the cascade origin — Task 4 Step 5/7 recompute from the first screendump if the resolution differs
  and dispatch the close via the deterministic guest `wm.close()` path as a fallback (the self-close
  reactor closes itself regardless of the click). (2) Memory budget: `MAX_WINDOWS=8` × (linear
  memory + surface) must fit the kernel heap — the reactor surface is only 320×240 (~0.3 MB), so the
  test is comfortable; a real 1280×800 app at the cap (~4 MB each) is the case to watch and is
  bounded by `MAX_WINDOWS`. (3) `Store` drop frees guest linear memory — verified indirectly by the
  reap marker; if a future leak is suspected, add a heap-free assertion around reap.
- **Consistency:** `WmState{id,win_w,win_h,pixels,tick,close_requested}`, the `wm.{commit,app_id,
  tick,close}` imports, the `frame` export, `APPS` (3 entries → `apps=3`), and the markers
  (`registry apps=3 modules_ok=3`, `lifecycle spawns=1 peak_live=1 final_live=0`,
  `reaped win_id=0`) are used identically across guest, host, spawn, reap, and the boot-checks.
