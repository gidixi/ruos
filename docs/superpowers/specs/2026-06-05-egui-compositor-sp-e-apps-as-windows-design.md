# egui apps in the compositor — SP-E: port the DeskApps as windows + retire gui.cwasm (design)

**Date:** 2026-06-05
**Status:** approved (brainstorm), pending spec review → writing-plans
**Branch:** `feat/egui-compositor-sp-e`

## Context — the arc + Model A

Goal: the compositor IS the desktop, grown as a userspace project. Done: SP-A (state
unification), SP-B (egui window + CSD), SP-C (`ruos-window` SDK + `wm.spawn`/`set_background`/
bg + WM shrink), SP-D (userspace desktop **shell** — wallpaper + panel + launcher → `wm.spawn`).
The launcher's "☰ Apps" menu already lists **egui-demo / About / Files / Terminal / System
Monitor** (the last four `wm.spawn` no-op because their `.cwasm` aren't shipped yet). SP-E
(this) ships those four as compositor windows + retires the full-screen `gui.cwasm`.

The four desktop apps already exist as `DeskApp`s in gui-core (`gui-core/src/desktop/apps/`):
`AboutRuos` (about.rs), `Files` (files.rs), `Terminal` (terminal.rs), `System` (system.rs —
the other-workstation System Monitor). `DeskApp` is a 3-method trait: `id()`, `title()`,
`ui(&mut self, &mut egui::Ui)`. All four are **pure egui with placeholder/simulated data**
(Files/Terminal are stubs; System's CPU/mem/process table are simulated, hardcoded — the
comment says "in ruos verrà dalla lista processi del kernel"). None touch the kernel/host.

**Decisions (brainstorm):**
- **Crate strategy A:** four thin cdylib crates (one `.cwasm` per app) — real per-process
  isolation, `wm.spawn(id)` → `/bin/<id>.cwasm` directly, "an app = a crate" growth. ~36 MB
  of extra `.cwasm` on disk (ISO ~115 MB); heap unchanged (only OPEN windows cost ~48 MB).
- **Port all four AS-IS** (real UI, placeholder/simulated data). **Real data** (System Monitor
  reading `proc::list`/CPU/mem; Terminal with a real PTY/shell) is a later **SP-F** (a kernel
  data host fn) — SP-E separates the porting from the data wiring.
- **Retire gui.cwasm** from the default build/ISO + the shell `gui` command; KEEP the
  `ruos-backend`/`Desktop` code in the submodule (git history + rebuildable), just unshipped.

## Goal (SP-E)

Clicking About / Files / Terminal / System Monitor in the desktop launcher opens that app
as an egui compositor window (its existing `DeskApp` UI, placeholder data); the four
windows are independent, draggable, closable. `gui.cwasm` no longer ships in the default
ISO and `gui` is no longer a launchable command. gui-core's `Desktop` path stays compilable.

## Architecture (crate strategy A)

### Part 1 — gui-core: expose the DeskApp structs (verify/minimal)

The four structs are already `pub` (`AboutRuos`, `Files`, `Terminal`, `System`) and the
modules `pub` (`desktop::apps::{about,files,terminal,system}`), so `gui_core::desktop::apps::
about::AboutRuos` etc. are reachable. SP-E **verifies** this (a host-target build that names
each from outside gui-core); if any struct/field needed by construction is not reachable,
make it `pub`. No behaviour change; `Desktop`/`panel` untouched.

### Part 2 — four thin window crates (on `ruos-window`)

Four new crates in the `ruos-desktop` workspace: `about-app`, `files-app`, `terminal-app`,
`system-app`. Each is a `wasm32-wasip1` cdylib reactor (like `compositor-app`/`shell`):

```rust
use ruos_window::{WindowState, frame_once};
use gui_core::desktop::apps::about::AboutRuos;   // (the app's DeskApp)
use gui_core::desktop::app_trait::DeskApp;

static mut S: Option<WindowState> = None;
static mut APP: Option<AboutRuos> = None;   // persists app state across frames

const W: u32 = 560; const H: u32 = 420;     // System gets 720×520

#[no_mangle]
pub extern "C" fn frame() {
    #[allow(static_mut_refs)]
    unsafe {
        if S.is_none() { S = Some(WindowState::new()); }
        if APP.is_none() { APP = Some(AboutRuos); } // construction per app (see below)
        let (s, app) = (S.as_mut().unwrap(), APP.as_mut().unwrap());
        let title = app.title();
        frame_once(s, title, W, H, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.ui(ui));
        });
    }
}
#[no_mangle] pub extern "C" fn _start() {}
```
- `frame_once` (the titlebar'd SDK variant) draws the CSD title bar (`app.title()`) + [X] +
  drag; the closure renders `app.ui(ui)` in a `CentralPanel`. The app's state (`Terminal.input`,
  `System.user_hist`/process sim) lives in the `static mut APP` across frames.
- Construction per app (from `default_apps()`): `AboutRuos` (unit struct → `AboutRuos`),
  `Files` (unit → `Files`), `Terminal::default()`, `System::default()`. Each crate's
  `static mut APP` holds the right type.
- The cwasm file name = the launcher id: `about-app` → `about.cwasm`, etc. (the Makefile names
  the output `/bin/<id>.cwasm` so `wm.spawn("about")` resolves).
- Sizes: About/Files/Terminal 560×420; System 720×520 (its table+charts need room). Fixed (resize
  is later).

### Part 3 — build + ship + launcher

- Four Makefile rules (`kernel/src/wasm/wt/{about,files,terminal,system}.cwasm`) mirroring the
  `shell.cwasm`/`egui_demo.cwasm` rule (build `-p <app>-app` wasm32-wasip1 → wt-precompile),
  shipped to `/bin/<id>.cwasm`, with a `limine.conf` module entry each (so the VFS mounts them →
  `wm.spawn(id)`/`module_by_name(id)` find them). The shell's CATALOG (SP-D) already lists the
  ids → the launcher entries now actually spawn.

### Part 4 — retire gui.cwasm

- Remove `gui.cwasm` from the `iso:`/`test-boot:` targets (the build rule + the `cp ... /bin/gui.cwasm`).
- Remove the shell `gui` command resolution if the shell exec router special-cases it (grep
  `gui.cwasm`/`"gui"` in the kernel exec/router; the compositor is the GUI now).
- KEEP `ruos-backend` + gui-core's `Desktop`/`panel` in the submodule (git history + rebuildable),
  just not shipped/launched. (A `RUOS_DESKTOP`-style make var or a comment documents how to rebuild
  it if needed.)

## Data flow

```
desktop shell launcher "☰ Apps" → click "About" → shell_chrome intents.launches=["about"]
  → ruos_window::spawn("about") → wm.spawn → kernel module_by_name("about") → /bin/about.cwasm
  → new window; about.frame() → frame_once(AboutRuos.ui) → CSD window with About content
```

## Error handling

- An app crate that traps in `frame()` → reaped (SP-B `frame_all` Err→close_requested). The
  window disappears; the launcher can respawn it.
- Heap pressure: shell + 4 app windows = 5 egui instances ≈ 5×48 MB = 240 MB < 256 MB heap —
  fits, but barely. Opening more (e.g. several egui-demos too) could OOM (bounded by
  `MAX_WINDOWS`); `wm.spawn` returns 0 / the spawn fails gracefully. Note for SP-F: tune heap or
  shrink the per-instance reservation if many windows are wanted.

## Testing / verification

1. **Build** — kernel (3 profiles) + the 4 app crates + ruos-window/gui-core → `.cwasm`. The
   `Desktop`/`ruos-backend` path still BUILDS (we only stop shipping gui.cwasm; we don't delete it).
2. **Boot-check (headless)** — optional/minimal: the apps need the VFS (not mounted in the
   interrupts phase) so the headless path can't `wm.spawn` them; assert the kernel builds + the
   shipped set (grep the iso recipe for about/files/terminal/system.cwasm). Rely on visual for the
   apps.
3. **Visual (QEMU+KVM)** — boot → desktop shell → "☰ Apps" → click each of About/Files/Terminal/
   System Monitor → its egui window opens (CSD titlebar + the app's content; System shows its
   table/charts). Open 2–3 at once → independent draggable/closable windows. Screendumps per app +
   a multi-window shot. Serial: `wm.spawn ok name='about'` etc.
4. **gui.cwasm gone** — the default ISO no longer contains `/bin/gui.cwasm`; `gui` is not a
   command. Confirm (grep the iso_root / try `gui` → not found).
5. **VBox** sanity (`[[vbox-test-harness]]`).

## Risks

- **Disk/ISO size:** +~36 MB of `.cwasm` (4×~9 MB). ISO ~115 MB — fine (built/tested in VM).
- **Window budget:** ~5 egui instances max at 256 MB heap; shell + 4 apps = 5 (at the limit).
  Note + defer heap tuning to SP-F if more concurrency is wanted.
- **`egui_extras::TableBuilder`** (System Monitor) must compile to wasip1 — it already does in
  gui.cwasm (same workspace pins), so OK.
- **Retiring gui.cwasm must not break the build:** keep `ruos-backend`/`Desktop` compilable
  (only stop shipping). If the Makefile's default `all`/`iso` hard-depends on gui.cwasm, decouple
  it cleanly.
- **App state in `static mut`:** each app crate's `static mut APP` is single-threaded-safe (the
  kernel calls `frame()` serially per window), mirroring `compositor-app`/`shell`.

## Out of scope (SP-E)

- **Real data** — System Monitor reading the kernel's `proc::list`/CPU/mem; Terminal with a real
  PTY/shell. That's **SP-F** (a `wm.sysinfo`-style kernel data host fn + adapting the app UIs).
- Window resize, a taskbar of open windows / window switching, minimize.
- Deleting `ruos-backend`/`Desktop` (kept as legacy).
- WIT-typed `wm`.

## Provides (for SP-F and beyond)

- The four desktop apps as real compositor windows + the "app = a thin `ruos-window` crate
  wrapping a `DeskApp`" pattern — adding an app = a new crate + a launcher CATALOG entry + a
  Makefile/limine line.
- The compositor as the sole GUI (gui.cwasm retired) — the desktop is now fully Model-A.
- The data-wiring need (SP-F): a kernel host fn feeding real CPU/mem/`proc::list` to System
  Monitor (replacing the simulation) + a real Terminal (PTY-in-window).
