# egui apps in the compositor — SP-D: userspace desktop shell (design)

**Date:** 2026-06-05
**Status:** approved (brainstorm), pending spec review → writing-plans
**Branch:** `feat/egui-compositor-sp-d`

## Context — the arc + Model A

Goal: the compositor IS the desktop, grown as a userspace project (Model A: kernel =
mechanism; desktop UX + apps = userspace). Done: SP-A (state unification), SP-B (egui
window + CSD), **SP-C** (the `ruos-window` SDK + kernel `wm.spawn`/`wm.set_background`/bg
window + WM shrunk to drop the kernel launcher). SP-D (this) builds the **userspace
desktop shell**; SP-E ports the apps (About/Files/Terminal/System Monitor) as windows +
retires `gui.cwasm`.

**Note (from the SP-C merge):** the other workstation added a **System Monitor app**
(CPU/memory/temperature) in `gui-core/src/desktop/apps/system.rs` (a `DeskApp`), now on
main. SP-E reuses it as a compositor window. SP-D just lists it in the launcher catalog.

The kernel today (post SP-C) boots the compositor into a single `egui-demo` window (no
kernel launcher). SP-D replaces that: the kernel boots into the **shell** (a full-screen
background window) that draws the panel + wallpaper + launcher; clicking a launcher entry
spawns that app as a separate compositor window via `wm.spawn`.

## Goal (SP-D)

A `shell.cwasm` userspace app, spawned by the kernel as the **background** window at GUI
start, draws the desktop chrome (wallpaper + top panel + launcher + clock, reusing
gui-core's look) and launches apps as compositor windows via `wm.spawn`. Booting the
compositor shows the desktop; clicking a launcher entry opens an app window on top.

## Architecture (approach A — reuse gui-core's panel + wallpaper)

### Part 1 — gui-core: a shell-chrome entry point (small refactor, stays pure)

gui-core's `Desktop::ui` does three things: top panel (`panel::show`), wallpaper
(`wallpaper::paint`), and internal egui-`Window` app rendering. SP-D needs only the first
two, with the launcher emitting *intents* instead of toggling internal windows.

- Add `pub fn shell_chrome(ctx: &egui::Context, apps: &[ShellAppEntry], clock_secs: f64)
  -> ShellIntents` to gui-core: draws the top panel (a launcher button per `apps` entry +
  the clock + a poweroff button) + the wallpaper, and returns
  `ShellIntents { launches: Vec<&'static str>, poweroff: bool }` (the ids of buttons
  clicked this frame + whether poweroff was pressed). `ShellAppEntry { id: &'static str,
  title: &'static str }`.
- This reuses `wallpaper::paint` verbatim and a panel-draw factored from `panel::show`
  (the launcher buttons + clock). gui-core stays **pure** (no `wm`/OS deps — it returns
  intents; the shell acts on them). The existing `Desktop` open-toggle path is **left
  intact** (gui.cwasm's full-screen desktop keeps working until SP-E retires it) — add
  `shell_chrome` ALONGSIDE, don't replace `panel::show`/`Desktop`.

### Part 2 — the `shell` crate (ruos-desktop, on `ruos-window`)

- A new `wasm32-wasip1` reactor crate `ruos-desktop/shell` depending on `ruos-window` +
  `gui-core` + `egui`. Its exported `frame()`:
  - On the FIRST frame, call `ruos_window::set_background()` (become the bg window) + size
    its surface to the full framebuffer via a new `wm.surface_size()` (the shell is
    full-screen, unlike the fixed-size apps).
  - Each frame: `gui_core::shell_chrome(ctx, CATALOG, clock)` → for each id in
    `launches` call `ruos_window::spawn(id)`; if `poweroff` call `ruos_window::poweroff()`.
  - It does NOT draw a CSD title bar (it's the desktop background, not a window) — so it
    uses **`ruos_window::frame_once_bare(state, w, h, ui)`** — a no-CSD-titlebar variant
    added to the SDK for the desktop background; the titlebar'd `frame_once` stays for app
    windows.
  - **The launcher CATALOG lives here** (the shell): a static list of `ShellAppEntry`
    (id/title) + the id→`/bin/<id>.cwasm` is the kernel's `wm.spawn` resolution. For SP-D:
    `[egui-demo, about, files, terminal, system]` — `egui-demo` spawns now (proof); the
    others spawn once SP-E ships their `.cwasm` (`wm.spawn` returns 0 = no-op meanwhile,
    graceful).

### Part 3 — kernel: boot-into-shell + `wm.poweroff` + `wm.surface_size`

- `Compositor::new` spawns `"shell"` (via `module_by_name("shell")`, fallback embedded if
  needed) as the initial window instead of `egui-demo`. The shell calls
  `wm.set_background()` on its first frame → becomes the full-screen bg.
- New host fns on the `wm` linker: `wm.poweroff()` (kernel power-off — reuse the existing
  poweroff path the `ruos:gui/power` desktop uses) and `wm.surface_size() -> (w,h)`
  (returns `gfx::geom()` width/height, so the bg shell sizes itself full-screen).
- Ship `shell.cwasm` to `/bin` (Makefile, like egui-demo). The launcher's app `.cwasm`
  (egui-demo now; About/Files/Terminal/System in SP-E) are also in `/bin`.

## Data flow

```
GUI boot: exec compositor → Compositor::new spawns "shell" (/bin/shell.cwasm) as the initial window
  shell.frame() frame 1: wm.set_background() + wm.surface_size() → full-screen bg
  shell.frame() each frame: shell_chrome(panel+wallpaper+launcher) → intents
LAUNCH: user clicks "egui-demo" in the panel → shell_chrome returns launches=["egui-demo"]
  → ruos_window::spawn("egui-demo") → wm.spawn → kernel loads /bin/egui-demo.cwasm → new window on top
POWEROFF: panel poweroff button → ruos_window::poweroff() → wm.poweroff() → machine off
```

## Error handling

- `wm.spawn(name)` for an app whose `/bin/<name>.cwasm` isn't shipped yet (SP-E pending) →
  returns 0, no window appears — the launcher button is a graceful no-op. No panic.
- The shell `frame()` trapping → reaped like any window, but the desktop background goes
  blank (no shell). For SP-D, acceptable; a future watchdog could respawn the shell.
- `wm.surface_size` before the framebuffer is up → returns 0×0; the shell defers sizing
  until non-zero (the compositor runs after `devices` init, so geom is valid).

## Testing / verification

1. **Build** — kernel (3 profiles) + the `shell` crate + ruos-window (bg/no-titlebar
   mode) + gui-core (`shell_chrome`) → `.cwasm`. gui.cwasm (full-screen Desktop) still
   builds (the `Desktop` path is untouched).
2. **Boot-check (headless)** — the kernel spawns `shell` as bg; assert a bg window exists
   full-screen. Marker `shell bg WxH=...`.
3. **Visual (QEMU+KVM)** — boot the compositor → the **desktop**: wallpaper + top panel
   with launcher buttons (egui-demo/about/files/terminal/system) + clock + poweroff, NO
   app window yet. Click **egui-demo** → an egui window opens on top (proves
   shell→`wm.spawn`). The look matches gui.cwasm's panel/wallpaper. Screendumps.
4. **VBox** sanity (`[[vbox-test-harness]]`).
5. **gui.cwasm regression** — the full-screen egui desktop still runs (its `Desktop` path
   is intact) — a quick `gui` exec or build check.

## Risks

- **gui-core refactor must not break `Desktop`/gui.cwasm:** add `shell_chrome` + a factored
  panel-draw alongside the existing `panel::show`/`Desktop::ui` — do NOT change their
  signatures. gui-core stays pure (intents, no wm dep).
- **Full-screen bg shell memory:** the shell is an egui instance (~48 MiB) + a full-screen
  surface (e.g. 1280×800×4 ≈ 4 MiB). With the 256 MiB heap, the shell + ~4 app windows fit
  (~5 egui instances). Many apps could OOM — bounded by `MAX_WINDOWS`; tune later.
- **bg window has no titlebar:** `ruos_window::frame_once` draws a CSD titlebar; the shell
  needs none (it's the desktop). Add `frame_once_bare` (no titlebar) to the SDK; the shell
  uses it.
- **Launcher catalog vs shipped apps:** listing apps whose `.cwasm` isn't shipped yet (SP-E)
  → `wm.spawn` no-ops. Either list only shipped apps (egui-demo) in SP-D, or list all + the
  others no-op until SP-E. Decision: list all (so the desktop looks complete); egui-demo
  proves the spawn. Document that the others arrive in SP-E.
- **`wm.poweroff` reuse:** wire it to the SAME kernel power-off the `ruos:gui/power.poweroff`
  desktop uses (don't reinvent).

## Out of scope (SP-D)

- The real apps (About/Files/Terminal/System Monitor) AS compositor windows — **SP-E**
  (each a `ruos-window` app wrapping the gui-core `DeskApp`'s `ui`).
- Retiring `gui.cwasm` / the full-screen Desktop — **SP-E**.
- A taskbar of OPEN windows / window switching / minimize — later (the panel is a launcher,
  not yet a full taskbar).
- WIT-typed `wm` — later.

## Provides (for SP-E)

- The shell + launcher: SP-E ships the real apps' `.cwasm` to `/bin`; they appear in the
  launcher (already listed) + spawn as windows.
- `wm.poweroff`/`wm.surface_size` + the bg/no-titlebar SDK mode.
- The `shell_chrome` intents pattern — SP-E's apps reuse gui-core's `DeskApp` UIs in
  `ruos-window` windows; once all apps are windows, `gui.cwasm`/the `Desktop` path is
  retired.
