# Compositor egui SP-B — egui-reactor harness + Client-Side Decorations — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Spec:** `docs/superpowers/specs/2026-06-05-egui-compositor-sp-b-csd-harness-design.md` (read first).

**Goal:** A minimal **egui** app (CSD title bar + label + counter button) spawns from the launcher and composites as a compositor window — its title bar, `[X]`, text and content all egui-rendered; drag the title bar → window moves; `[X]` → closes; click another window → focuses. `gui-core` (egui raster/input) reused; only a couple of items made `pub`.

**Architecture:** CSD — the kernel drops server-side decorations and becomes a pure compositor (composite raw surfaces + route input + window-control host fns). A new `wasm32-wasip1` reactor crate (`ruos-desktop/compositor-app`) implements `gui-core::Platform` over the `wm` host module, drives one egui frame per `frame()` export, and draws its own title bar. **Window move is kernel-driven** (Wayland-style interactive move): the app calls `wm.start_move()` on a title-bar grab and the kernel drags the window with the screen cursor, reusing SP3's `DragState`/`drag_to`.

**Tech Stack:** Rust nightly, kernel `no_std`; wasmtime 45 AOT (`Linker<AppState>` from SP-A). Guest: `wasm32-wasip1` reactor (`cdylib`, exports `frame`) depending on `gui-core` (egui 0.31 + tiny-skia, software raster). Build via WSL (`-d Ubuntu`, `/mnt/e/MinimalOS/BasicOperatingSystem`). Verify: kernel compile + headless boot-check + QEMU+KVM QMP screendump + VBox.

> **Refinement vs spec:** the spec said `wm.move(dx,dy)`; this plan uses **`wm.start_move()`** (kernel-driven interactive move, reusing SP3 drag) because an app-driven `move(dx,dy)` is circular — the app's pointer coords are window-local and shift as the window moves. Same user behaviour, cleaner mechanism.

---

## File Structure

| File | Responsibility |
|---|---|
| `kernel/src/wasm/wt/wm.rs` | CSD: `AppEntry.show_in_launcher`; `compose_window` = raw surface; `Window.rect` = whole window; drop `decor` drawing + `decor::hit`-based close/title from `on_left_down`; new host fns `wm.start_move`, `wm.wall_seconds`; trigger SP3 `DragState` from `wm.start_move`; `proc_exit`/trap → reap; embed `egui_demo.cwasm` + APPS entry. |
| `kernel/src/wasm/wt/wasi.rs` | `proc_exit` for a window store → `close_requested` (not trap). |
| `ruos-desktop/compositor-app/{Cargo.toml,src/lib.rs}` | NEW. wasip1 reactor: `Platform`-over-`wm` + egui ctx + CSD title bar + counter app + `frame()` export. |
| `ruos-desktop/gui-core/src/{raster.rs,input.rs,lib.rs}` | Make `Renderer`, `InputState` (+ `to_raw_input`) `pub` if not already (minimal exposure; no behaviour change). |
| `Makefile` | Build `compositor-app` → wasm32-wasip1 → `wt-precompile` → `kernel/src/wasm/wt/egui_demo.cwasm`; prereq + ship. |
| `kernel/src/wasm/wt/mod.rs` + `boot/phases/interrupts.rs` | Headless boot-check `egui demo spawn ok pixels=614400`. |

---

## Task 1: Retire demo reactors from the launcher (`show_in_launcher`)

**Files:** Modify `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: Add the field to `AppEntry`** and set it on every entry:

```rust
pub struct AppEntry {
    pub name: &'static str,
    pub cwasm: &'static [u8],
    pub show_in_launcher: bool,   // false = spawnable by name (boot-checks) but hidden from the taskbar
}
// In `pub static APPS`: the reactors get `show_in_launcher: false`; the egui demo (Task 6) gets `true`.
//   AppEntry { name: "react-A",     cwasm: REACTOR_CWASM,       show_in_launcher: false },
//   AppEntry { name: "react-B",     cwasm: REACTOR_CWASM,       show_in_launcher: false },
//   AppEntry { name: "selfclose",   cwasm: REACTOR_CLOSE_CWASM, show_in_launcher: false },
//   AppEntry { name: "wasip1-probe",cwasm: PROBE_CWASM,         show_in_launcher: false },
```

- [ ] **Step 2: Filter the launcher.** `draw_launcher` + `launcher_hit` must iterate ONLY entries with `show_in_launcher == true`, laying them out left-to-right by their position among the visible subset. Build a local `let visible: Vec<(usize, &AppEntry)> = APPS.iter().enumerate().filter(|(_, a)| a.show_in_launcher).collect();` in both, and map a clicked button index → the original `APPS` index for `spawn_app`. (Boot-checks call `spawn_app` by name via `APPS.iter().position(|a| a.name == ...)`, so they are unaffected.)

- [ ] **Step 3: Build + boot-check.**
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && source $HOME/.cargo/env && make test-boot ISO=build/cmtest.iso 2>&1 | tail -6 && grep -E "lifecycle spawns=1 peak_live=1 final_live=0|wasip1 probe spawn ok pixels=307200" build/test-boot.log'
```
Expected: lifecycle + wasip1 markers still pass (the boot-checks spawn by name; only the visible launcher changed — to empty for now, until Task 6 adds the egui demo).

- [ ] **Step 4: Commit.** `git add kernel/src/wasm/wt/wm.rs && git commit -m "feat(wm): AppEntry.show_in_launcher — retire demo reactors from the launcher"`

---

## Task 2: `wm.start_move` + `wm.wall_seconds` host fns (kernel-driven move)

**Files:** Modify `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: `wm.start_move`** — register in `add_to_linker` (the wm one). The calling store's window grabs an interactive move; the kernel drives it with the screen cursor (reuse SP3 `DragState`). The host fn knows the caller's window id (`caller.data().win.id`); it records a "move request" the run loop turns into a `DragState`:

```rust
// In add_to_linker<T: HasWindow>:
linker.func_wrap("wm", "start_move",
    |mut caller: Caller<'_, T>| { caller.data_mut().win().move_requested = true; })?;
```
Add `pub move_requested: bool` to `WmState` (init `false` at every `WmState { .. }` site — grep). In `Compositor::run`, AFTER `frame_all()` and BEFORE `present()`, scan windows: if `wins[i].store.data().win.move_requested`, clear it and begin a kernel drag of that window — set `self.drag = Some(DragState { win_id: wins[i].id, grab_dx: cursor_x - rect.0, grab_dy: cursor_y - rect.1 })` using `crate::gfx::mouse_pos()`. The existing mousemove→`drag_to` and button-up→`drag=None` machinery (SP3, still in `wm.rs`) then drives + ends the move. (No `decor` needed.)

- [ ] **Step 2: `wm.wall_seconds`** — egui needs a monotonic time for `RawInput.time` + animations:
```rust
linker.func_wrap("wm", "wall_seconds",
    |_caller: Caller<'_, T>| -> f64 { crate::wasm::wt::gfx::wall_secs() })?;
```
(`gfx::wall_secs()` is the latched monotonic source used by the desktop; confirm the exact path/name and reuse it.)

- [ ] **Step 3: Build.** `wsl ... cargo build --release ... --target x86_64-unknown-none` → `Finished` (host fns unused by current guests = fine).

- [ ] **Step 4: Commit.** `git commit -am "feat(wm): wm.start_move (kernel-driven interactive move) + wm.wall_seconds host fns"`

---

## Task 3: CSD compositor — raw-surface windows, drop decorations, crash-reap

**Files:** Modify `kernel/src/wasm/wt/wm.rs`, `kernel/src/wasm/wt/wasi.rs`.

- [ ] **Step 1: `compose_window` = raw surface.** Replace the SP3 decorated-footprint body so it returns the window's committed surface at its rect, with NO title bar band: `Some((win.store.data().win.pixels.clone(), rect.0, rect.1, win.store.data().win.win_w, win.store.data().win.win_h))` (guard empty pixels → None). `Window.rect` is now the WHOLE window. Remove the title-bar compositing + the `decor::TITLE_H` offset.

- [ ] **Step 2: Simplify `on_left_down`.** Remove the `decor::hit` Close/Title/Surface branching + the `DragState` start from a title hit. New body: `if let Some(i) = self.window_at(px,py) { let top = self.raise(i); self.set_focus(top); /* forward handled by run() routing the event to the focused window */ }`. (The app now owns `[X]`/title-bar via `wm.close`/`wm.start_move`.) Delete the now-dead `decor` drawing module + `decor::hit`/`window_rect`/`title_rect`/`close_rect` if nothing else uses them; keep any pure geometry still referenced. `draw_border`/decor draw already gone since SP4.

- [ ] **Step 3: Placement no longer needs `TITLE_H`.** In `Compositor::new`/`spawn_app`, drop the `sy >= TITLE_H` cascade offset; windows can be placed anywhere on-screen (still clamp to the framebuffer). Keep cascade so windows don't fully overlap.

- [ ] **Step 4: `proc_exit`/trap → reap.** In `kernel/src/wasm/wt/wasi.rs` `proc_exit`: if the store is a window (has `HasWindow` — but the closure is generic over `T: HasWasi`; instead route via a flag): set the exit code as today AND, because a windowed reactor must not poison its instance, do NOT trap — return `Ok(())` so `frame()` unwinds normally and the kernel reaps. Concretely: keep `proc_exit` setting `wasi().exit` and returning a trap for COMMAND apps (run_cwasm relies on it to stop `_start`), but for the compositor the simplest safe rule is in `frame_all`: treat a `frame()` that returns `Err` as `close_requested = true`:
```rust
// frame_all, per window:
match frame.call(&mut w.store, ()) {
    Ok(()) => {}
    Err(_) => { w.store.data_mut().win.close_requested = true; } // trap/panic/proc_exit → reap
}
```
This covers `proc_exit` (traps → Err), panics (`panic=abort` → trap → Err), and any guest fault — the window is reaped next loop instead of freezing. (Leave `proc_exit`'s command-app behaviour unchanged.)

- [ ] **Step 5: Build all 3 profiles + boot-check.** The existing reactors now composite **borderless** (no title bar) but still spawn/reap. `make test-boot ISO=build/cmtest.iso` → lifecycle + wasip1 markers still pass.

- [ ] **Step 6: Commit.** `git commit -am "feat(wm): CSD — raw-surface windows, drop kernel decorations, reap on frame() trap"`

---

## Task 4: `compositor-app` crate — Platform over `wm` + frame() reactor

**Files:** Create `ruos-desktop/compositor-app/{Cargo.toml,src/lib.rs}`; maybe `pub` in `gui-core`.

- [ ] **Step 1: Expose gui-core primitives if needed.** Check `ruos-desktop/gui-core/src/lib.rs`/`raster.rs`/`input.rs`: the new crate needs `gui_core::raster::Renderer` (+ its `render`) and `gui_core::input::InputState` (+ `to_raw_input`) to be `pub`. If they are `pub(crate)`, change to `pub` (no behaviour change). Confirm `gui_core::abi::{GfxEvent,GfxInfo}` and `gui_core::Platform` are already `pub` (they are — used by backends).

- [ ] **Step 2: `Cargo.toml`** (wasip1 reactor, depends on gui-core):
```toml
[package]
name = "compositor-app"
version = "0.0.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
gui-core = { path = "../gui-core" }
egui = "0.31"

[profile.release]
panic = "abort"
lto = true
```
Add `compositor-app` to the workspace `members` in `ruos-desktop/Cargo.toml`.

- [ ] **Step 3: `src/lib.rs`** — Platform over `wm` + egui driver + CSD title bar + counter. Imports the `wm` host module directly (raw extern, like the no_std reactor):
```rust
//! egui app as a compositor window (SP-B). wasip1 std reactor: exports frame(),
//! imports the `wm` surface protocol. Reuses gui-core's raster + input; draws its
//! OWN window (CSD title bar + content). The kernel composites the raw surface.

use gui_core::abi::{GfxEvent, GfxInfo, MouseButton};
use gui_core::input::InputState;
use gui_core::raster::Renderer;

#[link(wasm_import_module = "wm")]
extern "C" {
    fn commit(ptr: *const u8, len: u32, w: u32, h: u32);
    fn poll_event(ptr: *mut u8);     // 20-byte option<gfx-event> (SP2 ABI)
    fn app_id() -> u32;
    fn close();                      // wm.close
    fn start_move();                 // wm.start_move (kernel-driven drag)
    fn wall_seconds() -> f64;        // wm.wall_seconds
}

const W: u32 = 480;
const H: u32 = 320;

// Persistent per-instance state (single-threaded wasm; a static is fine).
struct App {
    ctx: egui::Context,
    input: InputState,
    renderer: Renderer,
    counter: u32,
}

// One global instance, lazily built on the first frame (after _initialize ran std).
static mut APP: Option<App> = None;

fn drain_events() -> Vec<GfxEvent> {
    let mut out = Vec::new();
    let mut buf = [0u8; 20];
    loop {
        unsafe { poll_event(buf.as_mut_ptr()); }
        let disc = u32::from_le_bytes([buf[0],buf[1],buf[2],buf[3]]);
        if disc == 0 { break; }
        let kind = u32::from_le_bytes([buf[4],buf[5],buf[6],buf[7]]);
        let p0 = u32::from_le_bytes([buf[8],buf[9],buf[10],buf[11]]);
        let p1 = u32::from_le_bytes([buf[12],buf[13],buf[14],buf[15]]);
        let ev = match kind {
            0 => GfxEvent::Key { scancode: p0, pressed: p1 != 0 },
            1 => GfxEvent::MouseMove { x: f32::from_bits(p0), y: f32::from_bits(p1) },
            2 => GfxEvent::MouseButton {
                button: match p0 { 0 => MouseButton::Left, 1 => MouseButton::Right, _ => MouseButton::Middle },
                pressed: p1 != 0,
            },
            _ => continue,
        };
        out.push(ev);
    }
    out
}

#[no_mangle]
pub extern "C" fn frame() {
    let app = unsafe {
        if APP.is_none() {
            APP = Some(App { ctx: egui::Context::default(), input: InputState::new(),
                             renderer: Renderer::new(), counter: 0 });
        }
        APP.as_mut().unwrap()
    };

    let info = GfxInfo { width: W, height: H, stride: W * 4, format: 0 };
    let events = drain_events();
    let raw = app.input.to_raw_input(&events, info);

    let mut counter = app.counter;
    let mut want_close = false;
    let mut want_move = false;
    let out = app.ctx.run(raw, |ctx| {
        // CSD title bar (reusable widget, Task 5 extracts it).
        egui::TopBottomPanel::top("titlebar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let bar = ui.label("egui demo");
                // Title-bar drag → kernel interactive move.
                if bar.drag_started() || ui.interact(ui.max_rect(), ui.id().with("tb"), egui::Sense::drag()).drag_started() {
                    want_move = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("✕").clicked() { want_close = true; }
                });
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(format!("window id {}", unsafe { app_id() }));
            if ui.button(format!("clicked {counter}")).clicked() { counter += 1; }
        });
    });
    app.counter = counter;
    if want_move { unsafe { start_move(); } }
    if want_close { unsafe { close(); } }

    // Tessellate + raster the FULL window each frame (dirty-rect inside Renderer).
    let prims = app.ctx.tessellate(out.shapes, 1.0);
    let (buf, _dirty) = app.renderer.render(&prims, &out.textures_delta, W, H);
    unsafe { commit(buf.as_ptr(), (W * H * 4) as u32, W, H); }
}

#[no_mangle]
pub extern "C" fn _start() {} // no-op: reactor, the kernel never run-to-completion calls it
```
> **NOTE:** the exact `Renderer::render`/`InputState::to_raw_input` signatures must match `gui-core` — mirror how `gui-core::Gui::frame` calls them (read `lib.rs` lines ~55-70). Adjust the `render` return (full buffer vs dirty crop) to commit a contiguous `W×H×4` buffer (the compositor expects `src_stride = W*4`). If `Renderer::render` returns a dirty sub-rect, commit the full canvas instead (the kernel composites the whole rect).

- [ ] **Step 4: Build the guest** (wasm32-wasip1) — confirm it compiles + imports:
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem/ruos-desktop && source $HOME/.cargo/env && cargo build --release -p compositor-app --target wasm32-wasip1 2>&1 | tail -15 && wasm-tools print compositor-app/target/wasm32-wasip1/release/compositor_app.wasm 2>/dev/null | grep -E "import \"(wm|wasi_snapshot_preview1)\"|export .*frame" | head'
```
Expected: `Finished`; imports `wm.{commit,poll_event,app_id,close,start_move,wall_seconds}` + `wasi_snapshot_preview1.*`; exports `frame`. (The wasip1 target/profile here mirror `ruos-backend`'s; if the workspace target dir differs, adjust the path.)

- [ ] **Step 5: Commit** (submodule). `cd ruos-desktop && git add Cargo.toml compositor-app gui-core && git commit -m "feat(compositor-app): egui CSD window over wm (Platform + frame reactor)"` then in the superproject `cd .. && git add ruos-desktop && git commit -m "chore: bump ruos-desktop (compositor-app egui CSD window)"`.

---

## Task 5: Extract the reusable CSD title-bar widget

**Files:** Modify `ruos-desktop/compositor-app/src/lib.rs` (split the title bar into a fn).

- [ ] **Step 1:** Factor the title-bar UI from Task 4 Step 3 into a reusable helper so SP-C reuses it:
```rust
/// Draw a CSD title bar. Returns (close_clicked, move_started). Apps call
/// wm.close()/wm.start_move() based on the result.
pub fn titlebar(ctx: &egui::Context, title: &str) -> (bool, bool) {
    let mut close = false; let mut mv = false;
    egui::TopBottomPanel::top("ruos_titlebar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            let resp = ui.add(egui::Label::new(title).sense(egui::Sense::drag()));
            if resp.drag_started() { mv = true; }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("✕").clicked() { close = true; }
            });
        });
    });
    (close, mv)
}
```
Use it in `frame()`: `let (want_close, want_move) = titlebar(&app.ctx, "egui demo");` inside the `ctx.run` closure (egui panels must be added during `run`). Adjust so `titlebar` is called within the closure or restructure to compute intents during `run`.

- [ ] **Step 2: Build the guest** again (Task 4 Step 4 command) → `Finished`.
- [ ] **Step 3: Commit** (submodule + superproject bump, as Task 4 Step 5).

---

## Task 6: Makefile build + embed `egui_demo.cwasm` + APPS entry

**Files:** Modify `Makefile`, `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: Makefile rule** (mirror the `gui.cwasm` rule ~line 121, but `-p compositor-app`):
```makefile
# egui CSD demo window (SP-B): compositor-app built wasm32-wasip1 (std, gui-core),
# precompiled to a CORE .cwasm embedded in the kernel.
EGUI_DEMO_SRCS := $(shell find ruos-desktop/compositor-app/src ruos-desktop/gui-core/src -name '*.rs' 2>/dev/null) \
                  ruos-desktop/compositor-app/Cargo.toml
kernel/src/wasm/wt/egui_demo.cwasm: $(EGUI_DEMO_SRCS) $(WT_PRECOMPILE)
	@mkdir -p build
	source $$HOME/.cargo/env && cd ruos-desktop && \
		cargo build --release -p compositor-app --target wasm32-wasip1
	$(WT_PRECOMPILE) ruos-desktop/compositor-app/target/wasm32-wasip1/release/compositor_app.wasm kernel/src/wasm/wt/egui_demo.cwasm
```
Add `kernel/src/wasm/wt/egui_demo.cwasm` as a prereq to `iso:` and `test-boot:`, and `cp` it to `$(ISO_ROOT)/bin/egui-demo.cwasm` in both. (Confirm the `compositor-app` wasm output path; the workspace may put it under `ruos-desktop/target/` not `compositor-app/target/` — adjust the rule to the real path from Task 4 Step 4.)

- [ ] **Step 2: Embed + APPS entry** in `wm.rs`:
```rust
static EGUI_DEMO_CWASM: &[u8] = include_bytes!("egui_demo.cwasm");
// add to APPS (show_in_launcher = true):
    AppEntry { name: "egui-demo", cwasm: EGUI_DEMO_CWASM, show_in_launcher: true },
```

- [ ] **Step 3: Build the kernel** (3 profiles) → `Finished` (include_bytes resolves).

- [ ] **Step 4: Commit.** `git add Makefile kernel/src/wasm/wt/wm.rs && git commit -m "build(wm): build + embed egui_demo.cwasm + launcher APPS entry"`

---

## Task 7: Headless boot-check — the egui app spawns + commits

**Files:** Modify `kernel/src/wasm/wt/wm.rs`, `kernel/src/wasm/wt/mod.rs`, `kernel/src/boot/phases/interrupts.rs`.

- [ ] **Step 1: Self-test** (mirror `wasip1_probe_self_test`): `Compositor::new_empty()`, spawn `"egui-demo"` by name, `frame_all()` (drives `run_initialize` + `frame()` — egui renders its first frame), return `wins.last().store.data().win.pixels.len()`.

```rust
pub fn egui_demo_self_test() -> usize {
    let mut c = Compositor::new_empty();
    let idx = APPS.iter().position(|a| a.name == "egui-demo").unwrap_or(usize::MAX);
    if idx == usize::MAX || c.spawn_app(idx).is_none() { return 0; }
    c.frame_all();
    c.wins.last().map(|w| w.store.data().win.pixels.len()).unwrap_or(0)
}
```

- [ ] **Step 2: mod.rs wrapper + interrupts marker** (`#[cfg(feature="boot-checks")]`): `egui demo spawn ok pixels={}`.

- [ ] **Step 3: Build + assert.** `make test-boot ISO=build/cmtest.iso` then `grep -E "egui demo spawn ok pixels=614400" build/test-boot.log` (480×320×4 = 614400). `pixels=0` → instantiate failed (a WASI import egui needs isn't registered — compare `wasm-tools print` imports vs `wasi.rs`; add it) or egui raster trapped (serial trap line). Report the value + import list.

- [ ] **Step 4: Commit.** `git commit -am "test(wm): headless boot-check — egui CSD demo spawns + commits"`

---

## Task 8: Visual verification (QEMU+KVM, then VBox)

**Files:** none (`build/egui_verify.py`).

- [ ] **Step 1:** `make iso INIT_SCRIPT=user-bin/compositor-init.sh ISO=build/comptest.iso`.
- [ ] **Step 2: QMP driver `build/egui_verify.py`** (model on `build/launch_verify.py`): boot headless; wait ~18s; `screendump build/egui-0-initial.png` (the launcher now shows ONE button "egui-demo"; the demo windows are gone/borderless); move to the egui-demo button + click → `screendump build/egui-1-spawned.png` (a window with an **egui title bar "egui demo" + ✕ + a "clicked 0" button**); move into the window + click the counter → `screendump build/egui-2-counter.png` (button reads "clicked 1" — egui input+state); move to the title bar + press-drag right+down + release → `screendump build/egui-3-moved.png` (window moved — `wm.start_move`); move to the ✕ + click → `screendump build/egui-4-closed.png` (window gone). Boot QEMU `-machine q35,accel=kvm:tcg -cpu max -m 512 -no-reboot -display none -serial file:build/egui-serial.log -qmp unix:/tmp/qmp.sock,server,nowait -device qemu-xhci -cdrom build/comptest.iso`.
- [ ] **Step 3:** Assert `grep "spawn app='egui-demo'" build/egui-serial.log`. Send the 5 PNGs to the controller to view (the real proof: egui titlebar + counter + move + close). If text doesn't render, re-check the SSE4.1 glyph path (CHANGELOG 265) holds for this guest.
- [ ] **Step 4: VBox** (VM `ruos`, `[[vbox-test-harness]]`): attach `build/comptest.iso`, boot headless, screenshot, confirm the egui window renders on HW-like; restore `os.iso`.
- [ ] **Step 5 (if a transition fails):** STOP + report which (spawn / counter / move / close) + the screendump + serial. Likely: a missing WASI import (egui uses more than the probe), a `Renderer::render` stride mismatch (commit a full `W*4`-stride buffer), or the title-bar drag not triggering `start_move` (check the egui drag sense).

---

## Task 9: Changelog + final review

- [ ] **Step 1:** Write `CHANGELOG/NN-26-06-05-egui-compositor-sp-b.md` (next free `NN` — currently `293`, so `294` unless taken). Summarise: CSD compositor (raw-surface windows, dropped decor, `wm.start_move`/`wm.wall_seconds`, `proc_exit`/trap→reap, `show_in_launcher`); the `compositor-app` egui crate (Platform over `wm` + frame reactor + CSD title bar + counter); verification (`egui demo spawn ok pixels=614400` + screendumps + VBox). Reference the spec + `[[vbox-test-harness]]` + the gui-core reuse.
- [ ] **Step 2:** Commit the changelog. Dispatch a final code-reviewer over the kernel CSD diff (`wm.rs`/`wasi.rs`) + the `compositor-app` crate, focusing on: (a) `frame_all` trap→reap doesn't reap a healthy window; (b) `wm.start_move` correctly grabs the calling window; (c) `compose_window` raw-surface bounds; (d) the egui driver commits a `W*4`-stride buffer; (e) no closure regressions from the host-fn additions.

---

## Provides (for SP-C)

- The `compositor-app` crate (Platform-over-`wm` + frame reactor + `titlebar()` widget) — SP-C's system-info app is another egui UI in the same crate/shape, reusing `titlebar()`.
- `wm.start_move`/`wm.close`/`wm.wall_seconds` + the CSD input model.
- SP-C adds the kernel data channel (`wm.sysinfo`-style host fn) the sandboxed egui app needs to read CPU/mem/`proc::list`.

## Self-Review notes
- **Spec coverage:** CSD compositor (spec Part 1) = Tasks 1,2,3; egui harness + titlebar + demo (Part 2) = Tasks 4,5,6; verification = Tasks 7,8; `wm.move`→`wm.start_move` refinement documented at top. Out-of-scope (resize/scroll/sysinfo) deferred.
- **Placeholders:** the egui driver + host fns + boot-check are shown in full; the two "confirm the exact gui-core signature / wasm output path" notes are explicit reconciliation points (the compiler + `wasm-tools print` are the checks), not vague TODOs.
- **Type consistency:** `AppEntry.show_in_launcher`, `WmState.move_requested`, `wm.{start_move,wall_seconds,close,commit,poll_event,app_id}`, `egui_demo.cwasm`/`EGUI_DEMO_CWASM`, marker `egui demo spawn ok pixels=614400` used identically across tasks.
