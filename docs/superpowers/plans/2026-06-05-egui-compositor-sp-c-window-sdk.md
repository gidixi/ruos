# Compositor egui SP-C — window-SDK + kernel mechanism (`wm.spawn` + background window) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

> **Spec:** `docs/superpowers/specs/2026-06-05-egui-compositor-sp-c-window-sdk-design.md` (read first).

**Goal:** Foundation for the Model-A desktop: a reusable **`ruos-window` SDK** (so an app = its egui UI + `frame_once`) + the kernel **mechanism** (`wm.spawn(name)` to launch an app by name from `/bin`, a **background-window** slot, and the WM **shrunk** to drop the kernel launcher). No shell, no real apps (SP-D/SP-E) — SP-C proves the mechanism + the SDK.

**Architecture:** Kernel = mechanism. `wm.spawn`/`wm.set_background` are window→kernel *requests* deferred to after `frame_all` (mirror the SP-B `close_requested`/`move_requested` pattern — never mutate `wins` mid-iteration). Spawn loads `/bin/<name>.cwasm` from the VFS (`vfs::block_on(crate::wasm::read_all(..))`, as the executor does) + a name-keyed module cache. The SDK extracts `compositor-app`'s reusable parts into a lib; the app keeps only the `#[no_mangle] frame()`/`_start` exports + its UI.

**Tech Stack:** kernel `no_std` wasmtime AOT (`Linker<AppState>`); guest `wasm32-wasip1` reactor depending on `ruos-window` + `gui-core`. Build via WSL (`-d Ubuntu`, `/mnt/e/MinimalOS/BasicOperatingSystem`). Verify: kernel compile + headless boot-check + QEMU QMP screendump + PC `pc-backend` dev + VBox.

---

## File Structure

| File | Responsibility |
|---|---|
| `kernel/src/wasm/wt/wm.rs` | `WmState` gains `spawn_request: Option<String>`, `bg_request: bool`; `Window` gains `bg: bool`. Host fns `wm.spawn`, `wm.set_background`. Run-loop: process spawn/bg requests after `frame_all`. `present`: composite `bg` first (full-screen, z-bottom). `on_left_down`/input: `bg` never raised/closed/moved; input fallthrough to `bg`. Remove `draw_launcher`/`launcher_hit` + kernel taskbar. `spawn_app` accepts a name-loaded module. Name-keyed module cache + VFS load. |
| `ruos-desktop/ruos-window/{Cargo.toml,src/lib.rs}` | NEW lib: `wm` extern bindings + `pub fn spawn/set_background/close/start_move`; `WindowState`; `pub fn frame_once(state, title, ui)`; `titlebar()`; `drain_events()`. Extracted from `compositor-app`. |
| `ruos-desktop/compositor-app/src/lib.rs` | Refactor to a THIN app on `ruos-window`: `static mut S` + `#[no_mangle] frame()` calling `frame_once` with the counter UI + a "spawn another" button (`ruos_window::spawn("egui-demo")`) + a "make background" toggle (`ruos_window::set_background()`). |
| `ruos-desktop/Cargo.toml` | Add `ruos-window` to `members`. |
| `kernel/src/wasm/wt/mod.rs` + `boot/phases/interrupts.rs` | Boot-check markers (`wm.spawn ok`, `bg window`). |
| `Makefile` | unchanged build path (compositor-app still → egui_demo.cwasm); ensure `/bin` has the spawnable `.cwasm` (egui-demo). |

---

## Task 1: `WmState`/`Window` request fields + name-keyed module cache + VFS load

**Files:** `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: Add request fields.** `WmState` gains `pub spawn_request: Option<alloc::string::String>` and `pub bg_request: bool` (init `None`/`false` at EVERY `WmState { .. }` literal — grep). `Window` gains `pub bg: bool` (init `false` at every `Window { .. }` literal — grep `spawn_app`, `new`, `new_empty`, spike).

- [ ] **Step 2: Name-keyed module loader.** Add a name-keyed cache + a VFS loader (the ptr-keyed `MODULE_CACHE` stays for embedded `APPS`; this is for `/bin`-loaded modules):
```rust
use alloc::string::String;
use alloc::collections::BTreeMap;
static NAME_CACHE: spin::Mutex<BTreeMap<String, Module>> = spin::Mutex::new(BTreeMap::new());

/// Load `/bin/<name>.cwasm` from the VFS and deserialize (cached by name). None on
/// any failure. Sync (the compositor loop owns the CPU): block on the async VFS read.
fn module_by_name(name: &str) -> Option<Module> {
    if let Some(m) = NAME_CACHE.lock().get(name) { return Some(m.clone()); }
    let path = alloc::format!("/bin/{}.cwasm", name);
    let bytes = crate::vfs::block_on(crate::wasm::read_all(&path)).ok()?;
    // SAFETY: wt-precompile output for this exact engine Config.
    let m = unsafe { Module::deserialize(engine(), &bytes) }.ok()?;
    NAME_CACHE.lock().insert(String::from(name), m.clone());
    Some(m)
}
```
(Confirm the exact sync-VFS API: `crate::wasm::read_all` is `async` — the executor `.await`s it; here use `crate::vfs::block_on(..)` per `state.rs`'s "synchronous via `crate::vfs::block_on`" note. If the real helper differs, adapt — grep `block_on`/`read_all`.)

- [ ] **Step 3: Build.** `wsl ... cargo build --release ... --target x86_64-unknown-none` → `Finished` (new fields/cache unused yet = warnings OK).

- [ ] **Step 4: Commit.** `git add kernel/src/wasm/wt/wm.rs && git commit -m "feat(wm): WmState spawn/bg request fields + name-keyed VFS module loader"`

---

## Task 2: `wm.spawn` + `wm.set_background` host fns + deferred processing

**Files:** `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: Host fns** in `add_to_linker<T: HasWindow>`:
```rust
// wm.spawn(name_ptr, name_len): request the kernel launch /bin/<name>.cwasm as a
// new window. Deferred to after frame_all (no wins mutation mid-iteration). The
// guest gets no return id here (fire-and-forget); a return-id variant is a later
// refinement. Reads the name from the calling guest's memory.
linker.func_wrap("wm", "spawn",
    |mut caller: Caller<'_, T>, name_ptr: i32, name_len: i32| {
        if let Some(b) = crate::wasm::wt::mem::read(&mut caller, name_ptr as u32, name_len as u32) {
            if let Ok(s) = core::str::from_utf8(&b) {
                caller.data_mut().win().spawn_request = Some(alloc::string::String::from(s));
            }
        }
    })?;
// wm.set_background(): the calling window flags itself as the background (full-screen,
// z-bottom, undecorated, not movable/closable). Deferred to the run loop.
linker.func_wrap("wm", "set_background",
    |mut caller: Caller<'_, T>| { caller.data_mut().win().bg_request = true; })?;
```

- [ ] **Step 2: Process requests in `Compositor::run`** — AFTER `frame_all()`, BEFORE `present()`, scan windows:
```rust
// Background requests: pin the window as bg.
for i in 0..self.wins.len() {
    if self.wins[i].store.data().win.bg_request {
        self.wins[i].store.data_mut().win.bg_request = false;
        self.wins[i].bg = true;
    }
}
// Spawn requests: collect names (drain the per-window request), then spawn (after the
// scan, so we don't borrow wins while pushing). Defers wins mutation past frame_all.
let mut to_spawn: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
for w in self.wins.iter_mut() {
    if let Some(name) = w.store.data_mut().win.spawn_request.take() { to_spawn.push(name); }
}
for name in to_spawn {
    if let Some(module) = module_by_name(&name) {
        let _ = self.spawn_named(&name, module); // Task 3: spawn_app variant taking a module
    } else {
        crate::bwarn!("wm", "wm.spawn: /bin/{}.cwasm not found/bad", name);
    }
}
```

- [ ] **Step 3: Build → `Finished`** (spawn_named added in Task 3; for this step stub it or reorder — do Task 3 Step 1 first if needed). Commit after Task 3.

---

## Task 3: `spawn_app` refactor (spawn by module) + WM shrink (drop launcher)

**Files:** `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: `spawn_named`.** Factor `spawn_app`'s instance-creation into `fn spawn_named(&mut self, name: &str, module: Module) -> Option<u32>` (alloc id, `Store::new(AppState{..})`, `run_initialize`, `linker.instantiate`, push `Window { .. bg: false }`, `raise`+`set_focus`, `proc::register`). Keep `spawn_app(idx)` (embedded `APPS`, for boot-checks) delegating to `spawn_named(app.name, module_for(app.cwasm)?)`.

- [ ] **Step 2: WM shrink.** Delete `draw_launcher` + `launcher_hit` + the call to `draw_launcher` in `run` + the `show_in_launcher` launcher iteration. The kernel no longer draws a taskbar (SP-D's shell does). Keep `AppEntry`/`APPS` ONLY as the boot-check spawn source (or drop `show_in_launcher`). `on_left_down`: keep `window_at`→`raise`→`set_focus`, but SKIP `bg` windows for raise/focus (a click on bare desktop hits the `bg` window for input but never raises/focuses it above apps).

- [ ] **Step 3: Initial window at boot (no launcher).** In `Compositor::new`, instead of the 2 demo reactors + launcher, spawn ONE initial window = the egui demo (`spawn_named("egui-demo", module_by_name("egui-demo")?)`), so `compositor` boots into the demo (the real shell-as-bg comes in SP-D). If `module_by_name` fails at construction (VFS not ready that early?), fall back to the embedded demo via `spawn_app`. (Confirm the VFS is mounted before `run_compositor_gate` runs — it is, the executor runs after fs init.)

- [ ] **Step 4: Build all 3 profiles → `Finished`.** Commit Tasks 2+3: `git add kernel/src/wasm/wt/wm.rs && git commit -m "feat(wm): wm.spawn/set_background + bg compositing + spawn_named; drop kernel launcher"`

---

## Task 4: `present`/input honor `bg` (full-screen bottom + fallthrough)

**Files:** `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: `present` composites bg first, full-screen.** In `present`, before the normal z-order loop, composite any `bg` window FIRST, forced to `(0,0,screen_w,screen_h)` (resize its rect to the framebuffer; the bg app commits at screen size — its `surface_info` will report screen size in SP-D; for SP-C the demo-as-bg may commit 480×320, so center/letterbox or just blit at (0,0) and leave the rest desktop-bg). Simplest: bg window rect = full framebuffer; `compose_window` already reads committed `win_w/win_h` — blit the bg surface at (0,0), then the desktop-bg fills any uncovered area (existing `DESKTOP_BG` clear stays).

- [ ] **Step 2: input fallthrough.** In the run loop's `window_at`, a click not inside any NON-`bg` window's rect routes to the `bg` window (so the shell's panel/launcher gets clicks). `bg` windows are never returned by `window_at` for raise/focus, only as the input fallback. A `bg` window is never moved (`wm.start_move` ignored for bg) or closed.

- [ ] **Step 3: Build → `Finished`.** Commit: `git commit -am "feat(wm): bg window full-screen bottom composite + input fallthrough"`

---

## Task 5: `ruos-window` SDK lib (extract from compositor-app)

**Files:** Create `ruos-desktop/ruos-window/{Cargo.toml,src/lib.rs}`; modify `ruos-desktop/Cargo.toml`.

- [ ] **Step 1: Crate.** `ruos-window/Cargo.toml`: a LIB (`crate-type=["lib"]`) depending on `gui-core` + `egui` (workspace pins). Add `"ruos-window"` to the workspace `members`.

- [ ] **Step 2: `src/lib.rs`.** Move from `compositor-app/src/lib.rs` (read it) the reusable parts:
  - the `#[link(wasm_import_module="wm")] extern` block — add `fn spawn(ptr,len)` + `fn set_background()` to the existing `commit/poll_event/app_id/close/start_move/wall_seconds`;
  - `pub fn spawn(name: &str)` (calls `spawn(name.as_ptr(), name.len())`), `pub fn set_background()`, `pub fn close()`, `pub fn start_move()` safe wrappers;
  - `pub fn titlebar(ctx, title) -> (bool,bool)` (verbatim);
  - `fn drain_events() -> Vec<GfxEvent>` (verbatim);
  - `pub struct WindowState { ctx: egui::Context, input: InputState, renderer: Renderer }` + `pub fn new(...)`;
  - `pub fn frame_once(state: &mut WindowState, title: &str, w: u32, h: u32, mut ui: impl FnMut(&egui::Context))` — the body of the current `frame()` (drain → to_raw_input → ctx.run{ titlebar + ui } → apply close/move intents → tessellate → render → commit), parameterized by the UI closure + size. NOTE: the wasm `#[no_mangle] frame`/`_start` exports CANNOT be in a lib — they stay in the app crate (Task 6).

- [ ] **Step 3: Build the lib for PC** (sanity, no_wasm): `cd ruos-desktop && cargo build -p ruos-window` (default host target) → `Finished`. (It's egui-only, no OS deps — compiles on host + wasip1.)

- [ ] **Step 4: Commit** (submodule): `cd ruos-desktop && git add Cargo.toml ruos-window && git commit -m "feat(ruos-window): window SDK extracted from compositor-app (frame_once + titlebar + wm bindings + spawn/set_background)"`

---

## Task 6: Refactor `compositor-app` onto the SDK + test hooks

**Files:** `ruos-desktop/compositor-app/{Cargo.toml,src/lib.rs}`.

- [ ] **Step 1: Depend on `ruos-window`** in `compositor-app/Cargo.toml`.

- [ ] **Step 2: Thin app.** Rewrite `compositor-app/src/lib.rs` to:
```rust
use ruos_window::{WindowState, frame_once, spawn, set_background};
static mut S: Option<WindowState> = None;
static mut COUNTER: u32 = 0;
static mut MADE_BG: bool = false;

#[no_mangle]
pub extern "C" fn frame() {
    #[allow(static_mut_refs)]
    let s = unsafe { if S.is_none() { S = Some(WindowState::new()); } S.as_mut().unwrap() };
    let counter = unsafe { &mut COUNTER };
    frame_once(s, "egui demo", 480, 320, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("egui demo window");
            if ui.button(format!("clicked {counter}")).clicked() { *counter += 1; }
            if ui.button("spawn another").clicked() { spawn("egui-demo"); }
            if ui.button("make background").clicked() { set_background(); }
        });
    });
}
#[no_mangle] pub extern "C" fn _start() {}
```
(`frame_once` draws the CSD titlebar + applies close/move; the closure adds the CentralPanel. The "spawn another"/"make background" buttons are the SP-C test hooks.)

- [ ] **Step 3: Build the guest** (`cargo build -p compositor-app --target wasm32-wasip1`) → `Finished`; `wasm-tools print` → imports include `wm.spawn` + `wm.set_background`; exports `frame`. Then `make kernel/src/wasm/wt/egui_demo.cwasm` + kernel build (3 profiles) → `Finished`.

- [ ] **Step 4: Commit** (submodule + superproject bump): `cd ruos-desktop && git add -A && git commit -m "refactor(compositor-app): thin app on ruos-window SDK + spawn/bg test hooks"` then `cd .. && git add ruos-desktop && git commit -m "chore: bump ruos-desktop (ruos-window SDK + thin compositor-app)"`

---

## Task 7: Boot-check + visual + PC dev + VBox

**Files:** `kernel/src/wasm/wt/wm.rs`, `mod.rs`, `boot/phases/interrupts.rs`; `build/spc_verify.py`.

- [ ] **Step 1: Boot-check.** `pub fn spawn_self_test() -> u32`: `Compositor::new_empty()`, `spawn_named("egui-demo", module_by_name("egui-demo")?)`, then simulate a spawn request (set the window's `spawn_request = Some("egui-demo")`), run the request-processing once, assert `wins.len() == 2`; mark one `bg` + assert its rect becomes full-screen after a `present`-prep. Return a flag bitset. mod.rs wrapper + `interrupts.rs` marker `wm spc: spawn ok wins=2 bg=WxH`.
- [ ] **Step 2: Build + assert** (`make test-boot ISO=build/cmtest.iso`): grep `wm spc: spawn ok wins=2` + lifecycle/wasip1 markers still pass.
- [ ] **Step 3: Visual (QEMU).** `make iso INIT_SCRIPT=user-bin/compositor-init.sh ISO=build/comptest.iso`; `build/spc_verify.py`: boot → the egui demo window (no kernel taskbar now); click "spawn another" → a 2nd egui window appears (`wm.spawn`); click "make background" on one → it fills the screen behind the other (`bg`). Screendumps `spc-0..2.png`; serial `wm.spawn`/bg. Send PNGs to the controller.
- [ ] **Step 4: PC dev path.** Confirm the demo's UI closure can run under `pc-backend` (or document the dev recipe): the UI is plain egui, so a small pc harness renders it in a window. (If `pc-backend` is wired to gui-core's `App`, note the recipe to run a `ruos-window` UI on PC; full PC harness for window-apps can be a tiny follow-up — the point is the UI is portable.)
- [ ] **Step 5: VBox** sanity (`[[vbox-test-harness]]`): boot comptest.iso, screenshot, confirm the demo renders + "spawn another" works (inject a click) or just boots clean; restore os.iso.

---

## Task 8: Changelog + final review

- [ ] **Step 1:** `CHANGELOG/NN-26-06-05-egui-compositor-sp-c.md` (next free NN — check `CHANGELOG/`, ~296). Summarize: `wm.spawn`(VFS name-load + deferred) + `wm.set_background` + bg compositing + WM shrink (dropped kernel launcher) + the `ruos-window` SDK + thin `compositor-app`. Verification markers + screendumps. Reference the spec + `[[vbox-test-harness]]`.
- [ ] **Step 2:** Commit the changelog. Dispatch a final code-reviewer over the kernel diff (`wm.spawn` re-entrancy/deferred-queue correctness, VFS load lifetime, bg compositing + input fallthrough, the launcher removal not breaking boot/spawn) + the `ruos-window` SDK + thin app.

---

## Provides (for SP-D / SP-E)
- `ruos-window` SDK (`frame_once` + `titlebar` + `spawn`/`set_background`/`close`/`start_move`) — SP-D's shell + SP-E's apps are thin crates on it.
- `wm.spawn(name)` + `/bin/<name>.cwasm` loader — SP-D's launcher calls `ruos_window::spawn(name)`.
- `wm.set_background()` + the bg full-screen-bottom mechanism — SP-D's shell calls `set_background()` at startup to become the desktop.
- The shrunk kernel WM (no launcher) — the desktop UX is now free to live in SP-D's userspace shell.

## Self-Review notes
- **Spec coverage:** `wm.spawn` (Tasks 1-3), bg window (Tasks 1,2,4), WM shrink (Task 3), `ruos-window` SDK (Task 5), thin compositor-app + test hooks (Task 6), verification incl PC dev (Task 7). Out-of-scope shell/apps deferred.
- **Placeholders:** kernel host fns + deferred processing + bg composite shown in full; the VFS-sync-read API + the PC-dev harness are explicit "confirm the real API"/"document the recipe" reconciliation points, not vague TODOs. SDK extraction references the read `compositor-app/src/lib.rs`.
- **Type consistency:** `spawn_request: Option<String>`, `bg_request: bool`, `Window.bg`, `module_by_name`, `spawn_named`, `frame_once`, markers `wm spc: spawn ok wins=2` — consistent across tasks.
