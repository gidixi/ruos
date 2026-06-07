# Compositor egui SP-D — userspace desktop shell — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

> **Spec:** `docs/superpowers/specs/2026-06-05-egui-compositor-sp-d-desktop-shell-design.md` (read first).

**Goal:** A userspace `shell.cwasm` that the kernel boots as the full-screen **background** window, drawing the desktop chrome (wallpaper + top panel + launcher + clock, reusing gui-core's look); clicking a launcher entry spawns that app as a compositor window via `wm.spawn`. The kernel boots into the shell instead of the egui-demo.

**Architecture (Model A, approach A):** gui-core gains a pure `shell_chrome(ctx, apps, clock) -> ShellIntents` (panel + wallpaper + launcher, returns launch/poweroff intents; `Desktop`/gui.cwasm path untouched). The `shell` crate (on `ruos-window`) runs it bare (no CSD titlebar — `frame_once_bare`), self-flags bg (`wm.set_background`), sizes full-screen (`wm.surface_size`), maps intents to `ruos_window::spawn(id)`/`poweroff()`. Kernel adds `wm.poweroff`/`wm.surface_size` + `Compositor::new` spawns `"shell"`.

**Tech Stack:** kernel `no_std` wasmtime AOT; guests `wasm32-wasip1` on `ruos-window`+`gui-core`+`egui`. Build via WSL (`-d Ubuntu`). Verify: kernel compile + headless boot-check + QEMU QMP screendump + VBox. `gui.cwasm` (full-screen Desktop) must still build/run.

---

## File Structure

| File | Responsibility |
|---|---|
| `ruos-desktop/ruos-window/src/lib.rs` | Add `wm.poweroff`/`wm.surface_size` externs + `pub fn poweroff()`/`pub fn surface_size() -> (u32,u32)`; add `pub fn frame_once_bare(state, w, h, ui)` (no CSD titlebar). |
| `ruos-desktop/gui-core/src/desktop/{shell.rs(new),mod.rs}` | `pub fn shell_chrome(ctx, apps, clock) -> ShellIntents` + `ShellAppEntry`/`ShellIntents` (panel launcher + wallpaper, intents). `panel::show`/`Desktop` UNCHANGED. |
| `ruos-desktop/shell/{Cargo.toml,src/lib.rs}` | NEW wasip1 reactor: `frame()` = first-frame `set_background`+`surface_size`; `frame_once_bare(shell_chrome(CATALOG))` → `spawn`/`poweroff`. The launcher CATALOG. |
| `kernel/src/wasm/wt/wm.rs` | Host fns `wm.poweroff` (→`crate::power::poweroff()`) + `wm.surface_size` (→`crate::gfx::geom()`, packed i64). `Compositor::new` spawns `"shell"` (was `egui-demo`). |
| `Makefile` + `limine.conf` | Build `shell.cwasm` (wasip1→wt-precompile) + ship to `/bin/shell.cwasm` + a `limine.conf` module entry (so the VFS has it, like egui-demo). |
| `kernel/src/wasm/wt/mod.rs` + `boot/phases/interrupts.rs` | Boot-check marker `shell bg WxH`. |

---

## Task 1: `ruos-window` — `wm.poweroff`/`wm.surface_size` + `frame_once_bare`

**Files:** `ruos-desktop/ruos-window/src/lib.rs` (submodule).

- [ ] **Step 1: externs + wrappers.** In the inner `mod wm` extern block add:
```rust
        pub fn poweroff();            // wm.poweroff
        pub fn surface_size() -> i64; // wm.surface_size → (w<<32)|h
```
Add public wrappers:
```rust
/// Power off the machine (the shell's power button).
pub fn poweroff() { unsafe { wm::poweroff() } }
/// Full framebuffer size (w,h). The bg shell sizes itself to this.
pub fn surface_size() -> (u32, u32) {
    let r = unsafe { wm::surface_size() };
    (((r >> 32) & 0xffff_ffff) as u32, (r & 0xffff_ffff) as u32)
}
```

- [ ] **Step 2: `frame_once_bare`.** Add a no-titlebar variant (the desktop bg has no CSD chrome). Copy `frame_once`'s body but DROP the `titlebar(...)` call + the close/move intent application (a bg window is never closed/moved by the user):
```rust
/// One egui frame for a BARE `w`×`h` window (no CSD title bar) — for the desktop
/// background shell. drain → RawInput → ctx.run(ui) → tessellate → raster → commit.
pub fn frame_once_bare(state: &mut WindowState, w: u32, h: u32, mut ui: impl FnMut(&egui::Context)) {
    let info = GfxInfo { width: w, height: h, stride: w * 4, format: GFX_FORMAT_RGBA8888 };
    let events = drain_events();
    let mut raw = state.input.to_raw_input(&events, info);
    raw.time = Some(unsafe { wm::wall_seconds() });
    let out = state.ctx.run(raw, |ctx| { ui(ctx); });
    let prims = state.ctx.tessellate(out.shapes, out.pixels_per_point);
    let (pixmap, _dirty) = state.renderer.render(&prims, &out.textures_delta, w, h);
    let data = pixmap.data();
    unsafe { wm::commit(data.as_ptr(), data.len() as u32, w, h); }
}
```

- [ ] **Step 3: Build the lib** (host + wasip1): `cd ruos-desktop && cargo build -p ruos-window && cargo build -p ruos-window --target wasm32-wasip1` → `Finished`.
- [ ] **Step 4: Commit** (submodule): `cd ruos-desktop && git add ruos-window && git commit -m "feat(ruos-window): wm.poweroff/surface_size wrappers + frame_once_bare (no-titlebar bg shell)"`.

---

## Task 2: gui-core — `shell_chrome` (pure, intents)

**Files:** Create `ruos-desktop/gui-core/src/desktop/shell.rs`; modify `desktop/mod.rs`.

- [ ] **Step 1: `shell.rs`.** A pure shell-chrome draw (launcher + wallpaper) returning intents. Reuses `wallpaper::paint` + `clock::format_hhmm`:
```rust
//! Shell chrome for the COMPOSITOR desktop (Model A): top panel launcher + wallpaper,
//! returning launch/poweroff INTENTS (no internal windows — the compositor owns
//! windows; the shell spawns apps via the host). Pure: no `wm`/OS deps. The full-screen
//! `Desktop` (gui.cwasm) keeps its own `panel::show`/window path unchanged.
use super::{clock, wallpaper};

/// A launchable app in the shell's launcher: id (→ /bin/<id>.cwasm) + display title.
#[derive(Clone, Copy)]
pub struct ShellAppEntry { pub id: &'static str, pub title: &'static str }

/// What the user asked for this frame.
pub struct ShellIntents { pub launches: alloc::vec::Vec<&'static str>, pub poweroff: bool }
// (gui-core is std on PC; use Vec via std::vec::Vec — match the crate's existing imports.)

/// Draw the desktop chrome (top panel launcher + wallpaper) and return intents.
pub fn shell_chrome(ctx: &egui::Context, apps: &[ShellAppEntry], clock_secs: f64) -> ShellIntents {
    let mut launches = Vec::new();
    let mut poweroff = false;
    egui::TopBottomPanel::top("ruos_shell_panel").show(ctx, |ui| {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("☰ Apps", |ui| {
                for a in apps {
                    if ui.button(a.title).clicked() { launches.push(a.id); ui.close_menu(); }
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⏻").clicked() { poweroff = true; }
                ui.label(clock::format_hhmm(clock_secs));
            });
        });
    });
    egui::CentralPanel::default().show(ctx, |ui| { wallpaper::paint(ui); });
    ShellIntents { launches, poweroff }
}
```
(Use the crate's `Vec` path — gui-core is std, so `Vec` is in scope; adjust the `alloc::vec::Vec` placeholder to plain `Vec`.)

- [ ] **Step 2: Register the module** in `desktop/mod.rs`: add `pub mod shell;` + `pub use shell::{shell_chrome, ShellAppEntry, ShellIntents};`. Do NOT change `panel`/`Desktop`.

- [ ] **Step 3: Build** (host): `cd ruos-desktop && cargo build -p gui-core` → `Finished` (gui.cwasm path intact).
- [ ] **Step 4: Commit** (submodule): `git add gui-core && git commit -m "feat(gui-core): shell_chrome (compositor desktop chrome → launch/poweroff intents); Desktop path unchanged"`.

---

## Task 3: the `shell` crate (wasip1 reactor)

**Files:** Create `ruos-desktop/shell/{Cargo.toml,src/lib.rs}`; add to workspace `members`.

- [ ] **Step 1: Cargo.toml** — cdylib, depends on `ruos-window` + `gui-core` + `egui` (workspace pins). Add `"shell"` to `ruos-desktop/Cargo.toml` members.

- [ ] **Step 2: `src/lib.rs`** — the bg shell reactor:
```rust
use ruos_window::{WindowState, frame_once_bare, set_background, surface_size, spawn, poweroff, app_id};
use gui_core::desktop::{shell_chrome, ShellAppEntry};

/// The launcher catalog: id (→ /bin/<id>.cwasm) + title. egui-demo spawns now; the
/// others spawn once SP-E ships their .cwasm (wm.spawn no-ops until then).
static CATALOG: &[ShellAppEntry] = &[
    ShellAppEntry { id: "egui-demo", title: "egui demo" },
    ShellAppEntry { id: "about",     title: "About" },
    ShellAppEntry { id: "files",     title: "Files" },
    ShellAppEntry { id: "terminal",  title: "Terminal" },
    ShellAppEntry { id: "system",    title: "System Monitor" },
];

static mut S: Option<WindowState> = None;
static mut INIT: bool = false;
static mut WH: (u32, u32) = (0, 0);

#[no_mangle]
pub extern "C" fn frame() {
    #[allow(static_mut_refs)]
    unsafe {
        if !INIT { set_background(); WH = surface_size(); INIT = true; }
        if S.is_none() { S = Some(WindowState::new()); }
        // Defer until the framebuffer geometry is valid (compositor runs post-devices).
        if WH.0 == 0 { WH = surface_size(); return; }
        let (w, h) = WH;
        let s = S.as_mut().unwrap();
        let clock = ruos_window::wall_seconds_pub(); // expose a wall_seconds() wrapper in the SDK if absent
        let mut launches: alloc::vec::Vec<&'static str> = alloc::vec::Vec::new();
        let mut want_poweroff = false;
        frame_once_bare(s, w, h, |ctx| {
            let intents = shell_chrome(ctx, CATALOG, clock);
            launches = intents.launches;
            want_poweroff = intents.poweroff;
        });
        for id in launches { spawn(id); }
        if want_poweroff { poweroff(); }
    }
}
#[no_mangle] pub extern "C" fn _start() {}
```
NOTE: `frame_once_bare`'s `wall_seconds` is internal; either expose `pub fn wall_seconds() -> f64` in `ruos-window` (Task 1) and call it for the clock, OR have `shell_chrome` receive the time differently. Add `pub fn wall_seconds() -> f64 { unsafe { wm::wall_seconds() } }` to ruos-window in Task 1 and use it here. (`app_id` import optional.)

- [ ] **Step 3: Build the guest** `cd ruos-desktop && cargo build --release -p shell --target wasm32-wasip1` → `Finished`; `wasm-tools print ruos-desktop/target/wasm32-wasip1/release/shell.wasm | grep -E 'import \"wm\"|export .*frame'` → imports `wm.{set_background,surface_size,spawn,poweroff,commit,poll_event,wall_seconds}`, exports `frame`.
- [ ] **Step 4: Commit** (submodule): `git add Cargo.toml shell && git commit -m "feat(shell): userspace desktop shell — bg window, panel launcher → wm.spawn, poweroff"`.

---

## Task 4: kernel — `wm.poweroff`/`wm.surface_size` + boot-into-shell

**Files:** `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: host fns** in `add_to_linker<T: HasWindow>`:
```rust
linker.func_wrap("wm", "poweroff", |_caller: Caller<'_, T>| { crate::power::poweroff(); })?;
linker.func_wrap("wm", "surface_size", |_caller: Caller<'_, T>| -> i64 {
    let g = crate::gfx::geom();
    ((g.width as i64) << 32) | (g.height as i64)
})?;
```
(`crate::power::poweroff()` is the same fn `ruos:gui/power.poweroff` uses — see `gui.rs`. `crate::gfx::geom()` returns `GfxGeom{width,height,..}`.)

- [ ] **Step 2: boot into shell.** In `Compositor::new`, change the initial-window spawn from `"egui-demo"` to `"shell"`: `spawn_named("shell", module_by_name("shell").or_else(|| /* fallback */)?)`. (The shell self-flags bg on its first frame. If `module_by_name("shell")` fails — shell.cwasm not in /bin — log + fall back to spawning egui-demo so the compositor still shows something.)

- [ ] **Step 3: Build all 3 profiles → `Finished`.** Commit: `git add kernel/src/wasm/wt/wm.rs && git commit -m "feat(wm): wm.poweroff/surface_size host fns + boot the compositor into the shell"`.

---

## Task 5: Makefile + limine — build + ship `shell.cwasm`

**Files:** `Makefile`, `limine.conf`.

- [ ] **Step 1: Makefile rule** `kernel/src/wasm/wt/shell.cwasm` (mirror the egui_demo.cwasm rule): build `-p shell` wasm32-wasip1 + `wt-precompile`. Prereq on `iso:`/`test-boot:` + `cp ... $(ISO_ROOT)/bin/shell.cwasm`. (Or build straight to `$(ISO_ROOT)/bin/shell.cwasm`.)
- [ ] **Step 2: limine.conf module** — add a `/bin/shell.cwasm` module entry (like egui-demo) so the VFS `/bin` has it (the SP-C fix: `wm.spawn`/`module_by_name` reads from the VFS, populated by limine boot-modules).
- [ ] **Step 3: Build** `wsl ... make kernel/src/wasm/wt/shell.cwasm` + kernel + `make iso INIT_SCRIPT=user-bin/compositor-init.sh ISO=build/comptest.iso` → ISO written.
- [ ] **Step 4: Commit.** `git add Makefile limine.conf && git commit -m "build: build + ship + mount shell.cwasm"`.

---

## Task 6: Boot-check + visual + VBox

**Files:** `kernel/src/wasm/wt/wm.rs`, `mod.rs`, `boot/phases/interrupts.rs`; `build/spd_verify.py`.

- [ ] **Step 1: Boot-check.** `spd_self_test()`: build a `Compositor::new_empty()`, spawn `shell` (embedded fallback or module_by_name if VFS ready — for headless use the EMBEDDED path: ship shell.cwasm embedded via include_bytes for the boot-check, OR skip the headless shell spawn and rely on visual). Mark its window bg + assert full-screen. Marker `shell bg WxH=...`. (If the headless VFS-load is awkward, assert only the host fns exist + the bg mechanism from SP-C; rely on visual for the shell.)
- [ ] **Step 2: Visual (QEMU).** `make iso INIT_SCRIPT=user-bin/compositor-init.sh ISO=build/comptest.iso`; `build/spd_verify.py`: boot → `screendump build/spd-0-desktop.png` (the DESKTOP: wallpaper + top panel "☰ Apps" + clock + ⏻, NO app window). Click "☰ Apps" → click "egui demo" → `screendump build/spd-1-launched.png` (an egui window opens on top). Serial: `wm.spawn ok name='egui-demo'`. Send PNGs to the controller.
- [ ] **Step 3: VBox** sanity (`[[vbox-test-harness]]`): boot comptest.iso, screenshot → the desktop shell renders; restore os.iso.
- [ ] **Step 4: gui.cwasm regression** — `make iso` (default) still builds gui.cwasm (the Desktop path); a quick boot or build confirms it's intact.

---

## Task 7: Changelog + final review

- [ ] **Step 1:** `CHANGELOG/NN-26-06-05-egui-compositor-sp-d.md` (next free NN — ~298). Summarize: gui-core `shell_chrome` (pure intents; Desktop intact); the `shell` crate (bg window, panel launcher → `wm.spawn`, poweroff); kernel `wm.poweroff`/`wm.surface_size` + boot-into-shell; `frame_once_bare`. Verification + screendumps. Reference spec/plan + `[[vbox-test-harness]]`.
- [ ] **Step 2:** Commit. Final code-reviewer over the kernel host fns + boot-into-shell + the `shell` crate + gui-core `shell_chrome` (purity, no Desktop regression, full-screen sizing, intents wiring).

---

## Provides (for SP-E)
- The shell + launcher catalog: SP-E ships About/Files/Terminal/System `.cwasm` to `/bin`; they're already listed → spawn as windows.
- `wm.poweroff`/`wm.surface_size` + `frame_once_bare`.
- The pattern: an app = a `ruos-window` crate wrapping a gui-core `DeskApp`'s `ui`. SP-E wraps the existing `DeskApp`s (incl. the other-PC `system.rs` System Monitor) as window crates; once all are windows, `gui.cwasm`/`Desktop` retires.

## Self-Review notes
- **Spec coverage:** shell_chrome (Task 2), shell crate (Task 3), wm.poweroff/surface_size + boot-into-shell (Task 4), frame_once_bare (Task 1), ship/mount (Task 5), verify incl gui.cwasm regression (Task 6). Out-of-scope apps deferred to SP-E.
- **Placeholders:** the new host fns + frame_once_bare + shell_chrome + the shell frame() are shown in full; the "expose wall_seconds wrapper" + "headless boot-check vs visual" are explicit reconciliation notes; the `Vec` path note flags gui-core-is-std. SDK extraction references the read ruos-window/panel sources.
- **Type consistency:** `ShellAppEntry{id,title}`, `ShellIntents{launches,poweroff}`, `shell_chrome`, `frame_once_bare`, `wm.poweroff`/`wm.surface_size` (packed i64), marker `shell bg WxH` — consistent across tasks.
