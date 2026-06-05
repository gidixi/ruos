# egui apps in the compositor — SP-C: window-SDK + kernel mechanism (`wm.spawn` + background window) (design)

**Date:** 2026-06-05
**Status:** approved (brainstorm), pending spec review → writing-plans
**Branch:** `feat/egui-compositor-sp-c`

## Context — the arc + Model A

Goal: **the compositor becomes THE desktop, and grows as a userspace project** (like
ruos-desktop). **Architecture decision (Model A):** the kernel WM is pure *mechanism*
(composite surfaces, route input, spawn/kill wasm instances, window-control host fns);
all desktop *policy/UX* (wallpaper, top panel, launcher, clock, the app catalog) and the
apps live in a growable userspace project. The full-screen `gui.cwasm` desktop is retired
in favour of egui *windows*.

Done so far: SP-A (state unification — `Linker<AppState>` = WASI + `wm`), SP-B (first egui
window + Client-Side Decorations; the `compositor-app` crate). Remaining: **SP-C (this) —
the window-SDK + the kernel mechanism** → SP-D (userspace desktop shell `shell.cwasm`) →
SP-E (port About/Files/Terminal apps + sysinfo + retire `gui.cwasm`).

SP-C is the **foundation**: the reusable window SDK every app/shell uses, plus the kernel
mechanism (`wm.spawn`, the background-window concept) SP-D/SP-E depend on. SP-C ships **no
shell and no real apps** — it proves the mechanism + the SDK.

## Goal (SP-C)

1. A reusable **window SDK** (`ruos-window` crate in `ruos-desktop`): the `Platform`-over-`wm`
   impl + the CSD `titlebar()` widget + the per-frame egui driver + the `wm` extern
   bindings, so an app is a tiny crate = its egui UI + `SDK::run(ui)`. The app's UI stays
   portable (egui), runnable on PC via `pc-backend` for fast dev.
2. A kernel host fn **`wm.spawn(name)`**: a window asks the kernel to launch another app by
   name; the kernel loads `/bin/<name>.cwasm` from the VFS, instantiates it as a new window,
   returns its id. (This is how SP-D's shell launches apps.)
3. A **background-window** mechanism: a window flagged `bg` is pinned full-screen at the
   bottom of the z-order, undecorated, not movable/closable (the slot SP-D's shell fills).
4. The kernel WM **shrinks**: the kernel-drawn launcher/taskbar (`draw_launcher`/
   `launcher_hit`) and the embedded `APPS`-as-launcher catalog are removed — the catalog +
   launcher UX move to the userspace shell (SP-D).

## Architecture

### Part 1 — Kernel mechanism (`kernel/src/wasm/wt/wm.rs`)

- **`wm.spawn(name_ptr: i32, name_len: i32) -> i32`** host fn: read the UTF-8 app name from
  the calling guest's memory; load `/bin/<name>.cwasm` from the VFS (the same way the
  executor already reads a `.cwasm` for shell exec); deserialize the module (cache by
  *name*, since VFS bytes are not `&'static` — a `BTreeMap<String, Module>` keyed by name,
  alongside or replacing the ptr-keyed `MODULE_CACHE`); build a new window via the existing
  `spawn_app` path; return the new window id (0 = failure: bad name / missing file / bad
  module / budget full). Any window may call it (single-user hobby OS; restricting to the
  shell is a later concern).
- **Background window:** add `pub bg: bool` to `Window`. A `bg` window is composited FIRST
  (z-bottom), forced to the full framebuffer size + origin (0,0), undecorated (the app draws
  no titlebar / the kernel never treats it as draggable/closable), and receives input only
  where no non-`bg` window covers the point. `raise`/`set_focus`/close never apply to a `bg`
  window. SP-C adds the mechanism + **`wm.set_background()`** — a host fn a window
  calls on itself (first frame) to flag itself `bg`; SP-D's shell calls it at startup. SP-C
  does NOT auto-spawn a shell at boot (that's SP-D) — it verifies the mechanism with a test
  window that calls `wm.set_background()`.
- **WM shrink:** remove `draw_launcher` + `launcher_hit` + the use of the embedded `APPS`
  slice as the launcher catalog + the kernel taskbar drawing. Keep: the window list,
  compositing/`present`, input routing, CSD (`wm.start_move`/`close`/focus), and `spawn_app`
  (refactored so it can build a window from a name-loaded module, not only an embedded
  `AppEntry`). The boot-check guests (reactor/probe/egui-demo) stay spawnable by name for
  tests; the embedded `APPS` registry may remain ONLY as the boot-check/spawn source, not as
  a launcher.

### Part 2 — Window SDK (`ruos-desktop/ruos-window` crate)

- A new library crate `ruos-window` in the `ruos-desktop` workspace. It extracts from
  `compositor-app` the reusable parts: the `Platform`-over-`wm` impl (`present→wm.commit`,
  `poll_events→wm.poll_event`, `surface_info→` a size the app picks, `wall_clock_secs→
  wm.wall_seconds`), the CSD `titlebar()` widget, the per-frame egui driver (poll → input →
  `ctx.run` → tessellate → `gui_core::raster::Renderer` → commit; the SP-B/EX3 input plumbing),
  and the `wm` extern bindings (`commit/poll_event/app_id/close/start_move/wall_seconds/spawn`).
- **App shape:** an app = a tiny `wasm32-wasip1` reactor bin that provides an egui UI (a
  closure `FnMut(&egui::Context)` or a small `WindowApp` trait) + `ruos_window::run(title, ui)`
  which exports `frame()` and drives the SDK. So adding an app = a small crate, no kernel
  change.
- **Portable UI / PC dev:** the app's UI is plain egui (no OS deps), so the SAME UI runs on
  PC via `pc-backend` (winit) for fast iteration — the "grows on its own" loop, exactly like
  gui.cwasm was developed. `ruos-window` is the ruos backend; `pc-backend` is the dev backend;
  the UI is shared.
- **Refactor `compositor-app`:** the SP-B demo becomes a thin app using `ruos-window` (its
  counter UI + `run`), proving the SDK + serving as the SP-C test app (it gets a "spawn
  another" button → `wm.spawn("egui-demo")`, and can mark itself `bg` to test the background
  mechanism).

## Data flow

```
SPAWN: window calls wm.spawn("egui-demo")
  → kernel: read /bin/egui-demo.cwasm (VFS) → module_for_name("egui-demo") (cache by name)
  → spawn_app(module) → new Window pushed, raised, focused → returns id
BACKGROUND: a window marked bg
  → present(): composite bg window FIRST at (0,0,screen_w,screen_h), then app windows on top
  → input: a point not covered by any non-bg window routes to the bg window
```

## Error handling

- `wm.spawn` returns 0 on any failure (missing `/bin/<name>.cwasm`, bad module, budget full);
  the caller (shell) handles 0 gracefully (no window appears). No kernel panic.
- A `bg` window that returns `Err` from `frame()` (trap/panic) — reap it like any window, but
  the desktop background goes blank until re-spawned (SP-D decides whether to respawn the
  shell). For SP-C, just reap.
- `wm.spawn` re-entrancy: the spawn happens from within a window's `frame()` (host fn called
  during `frame_all`); the kernel must defer the actual instance creation to AFTER the
  current `frame_all` pass (a spawn-request queue, like the SP-B `move_requested`/
  `close_requested` pattern) to avoid mutating `wins` mid-iteration.

## Testing / verification

1. **Build** — kernel (3 profiles) + the `ruos-window` lib + the refactored `compositor-app`
   → `.cwasm`. The shrink doesn't break the existing boot-checks (they spawn by name).
2. **Boot-check (headless)** — a window spawns another via `wm.spawn` (e.g. spawn egui-demo,
   then from it spawn a second), assert 2 live windows; a `bg`-marked window reports
   full-screen rect. Markers `wm.spawn ok id=N`, `bg window WxH=...`.
3. **Visual (QEMU+KVM)** — boot the compositor (SP-C has no shell; init spawns the demo as
   the initial window): the demo's "spawn another" button → a second window appears
   (`wm.spawn`); a window marked `bg` fills the screen behind the others. Screendump.
4. **PC dev** — the demo's UI runs in a PC window via `pc-backend` (the dev loop is intact).
5. **VBox** sanity (`[[vbox-test-harness]]`).

## Risks

- **VFS module load + cache:** loading `.cwasm` from the VFS at runtime (not `include_bytes!`)
  + caching by name is new; the ptr-keyed `MODULE_CACHE` must coexist or be replaced by a
  name-keyed one. Watch lifetime (the loaded `Vec<u8>` must outlive deserialize; `Module`
  owns its code after `deserialize`).
- **Spawn re-entrancy:** spawning from inside `frame()` must defer to after `frame_all`
  (queue), else `wins` is mutated mid-iteration (UB/panic). Mirror the `close_requested`
  pattern.
- **SDK extraction churn:** moving `compositor-app`'s guts into `ruos-window` + refactoring
  the demo to use it; keep the SP-B input fixes (positioned mouse, hit-rect=surface) in the SDK.
- **Background input:** the "input only where no app window covers" rule must be correct so
  the shell's panel/launcher (SP-D) gets clicks not eaten by the bg fallthrough.
- **WM shrink regressions:** removing the kernel launcher must not break the boot-checks or
  the `compositor` command (which now boots into the demo, not a launcher, until SP-D).
- **Submodule:** `ruos-window` + `compositor-app` changes are in the `ruos-desktop` submodule;
  coordinate the submodule commit + the kernel embed/ship.

## Out of scope (SP-C)

- The actual desktop shell UI (wallpaper/panel/launcher/clock) — **SP-D**.
- Porting About/Files/Terminal + sysinfo — **SP-E**.
- Auto-spawning a shell as the background at GUI boot — **SP-D**.
- Retiring `gui.cwasm` — **SP-E**.
- A WIT-typed `wm` interface — later.

## Provides (for SP-D / SP-E)

- `ruos-window` SDK: `run(title, ui)` to make any egui UI a compositor window; the
  `titlebar()` widget; the PC dev path. SP-D's shell + SP-E's apps are thin crates on it.
- `wm.spawn(name)` + the name→`/bin/<name>.cwasm` loader — SP-D's launcher calls it.
- The `bg` background-window mechanism — SP-D's shell runs as the `bg` window.
- The shrunk kernel WM (no launcher/catalog) — the policy is now free to live in SP-D's shell.
