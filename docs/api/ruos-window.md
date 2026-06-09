# `ruos-window` — the app-author API

**Start here.** This is the crate a GUI app actually uses. It wraps the raw
[`wm`](wm.md) / [`sys`](sys.md) / [`term`](term.md) host modules behind a small safe
API: a per-frame egui driver, the launcher manifest macro, and helpers. You build
egui UI; this crate does input → raster → commit and draws the window's title bar.

Source of truth: `ruos-desktop/crates/ruos-window/src/lib.rs` + `crates/gui-core`.
The SDK pulls these into `vendor/ruos-desktop`. UI widgets are plain
[`egui` 0.31](https://docs.rs/egui/0.31) — its docs apply verbatim inside the
`frame_once` closure.

**Last reviewed:** 2026-06-09.

---

## The app shape

A GUI app is a `cdylib` exporting exactly three symbols:

```rust
use ruos_window::{frame_once, WindowState};

// 1. Launcher entry. id MUST equal the .cwasm stem (the spawn key).
ruos_window::declare_manifest!("myapp", "My App", 640, 480);

const W: u32 = 640;
const H: u32 = 480;

// 2. Per-window persistent state (one instance per window process).
static mut S: Option<WindowState> = None;

// 3. Called by the compositor each frame. Build egui here.
#[no_mangle]
pub extern "C" fn frame() {
    #[allow(static_mut_refs)]
    unsafe {
        if S.is_none() { S = Some(WindowState::new()); }
        let s = S.as_mut().unwrap();
        frame_once(s, "My App", W, H, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("Hello");
            });
        });
    }
}

// 4. wasip1 reactor init — nothing to do; the compositor drives frame().
#[no_mangle]
pub extern "C" fn _start() {}
```

`Cargo.toml`: `[lib] crate-type = ["cdylib"]`, deps `ruos-window` (path) + `egui`.

---

## Core API

### `WindowState`
Opaque per-window state (egui `Context` + input + software renderer). Create once,
lazily, on the first `frame()`:
```rust
pub struct WindowState { /* private */ }
impl WindowState { pub fn new() -> Self }
```
Keep it in a `static mut Option<WindowState>` (single-threaded; one window = one
process). Store your own app state in separate statics or a struct held in an
`Option`.

### `frame_once`
```rust
pub fn frame_once(
    state: &mut WindowState,
    title: &str,
    w: u32, h: u32,
    ui: impl FnMut(&egui::Context),
)
```
Runs one egui frame: drains input → builds the CSD **title bar** (traffic-light
close/min/max + drag, wired automatically) → runs your `ui` closure → tessellates →
rasters (tiny-skia) → commits the surface. Behaviour to rely on:

- **Sizing**: if the kernel assigned this window a size (maximize/restore), it
  renders at THAT size; otherwise at your `(w, h)` and the kernel adopts the first
  committed size. Read [`window_size()`](#window_size) if you need the current size.
- **Title bar is automatic** — do NOT draw your own; just fill the `CentralPanel`.
  Its close/minimize/maximize/move actions are applied for you.
- **Commit-on-damage**: if nothing changed, no surface is pushed (idle windows cost
  nothing). For continuous animation call [`stay_awake()`](#stay_awake) each frame.
- `ui` receives `&egui::Context`; typically `egui::CentralPanel::default().show(ctx, |ui| …)`.

### `frame_once_bare`
```rust
pub fn frame_once_bare(state: &mut WindowState, w: u32, h: u32, ui: impl FnMut(&egui::Context))
```
Same pipeline WITHOUT the title bar — for the desktop background shell
(see [`set_background`](#set_background)). Normal apps use `frame_once`.

### `declare_manifest!`
```rust
ruos_window::declare_manifest!("<id>", "<Title>", <w>, <h>);
```
Emits the `manifest() -> i64` export the compositor scans for (in `/bin` +
`/mnt/apps`). **`<id>` MUST equal the `.cwasm` stem** — it is the `spawn` key and
the file name. Width/height are the app's default window size.

---

## Helpers (safe wrappers over `wm`)

| Function | Effect |
|----------|--------|
| `spawn(name: &str)` | Launch `/bin/<name>.cwasm` as a new window (fire-and-forget). |
| `close()` | Close THIS window. |
| `minimize()` | Minimize to the taskbar. |
| `toggle_maximize()` | Maximize to work-area / restore. |
| `activate(id: u32)` | Un-minimize + raise + focus window `id`. |
| `start_move()` | Begin interactive drag (the title bar already calls this). |
| `set_background()` | Flag THIS window as the desktop background (use `frame_once_bare`). |
| `stay_awake()` | Request a repaint next frame (call every frame for animation). |
| `poweroff()` | Power off the machine. |
| `app_id() -> u32` | THIS window's id. |
| `surface_size() -> (u32, u32)` | Full screen size; `(0,0)` before the framebuffer is up. |
| `window_size() -> (u32, u32)` | THIS window's kernel size; `(0,0)` until established. |
| `wall_seconds() -> f64` | Monotonic seconds since boot (for clocks/animation). |
| `window_list() -> Vec<TaskbarWindow>` | Non-bg windows (id, minimized, focused, title) — for a taskbar. |
| `app_list() -> Vec<AppCatalogEntry>` | Launchable apps (id, title) found by the manifest scan — for a launcher. |

```rust
pub struct TaskbarWindow { pub id: u32, pub minimized: bool, pub focused: bool, pub title: String }
pub struct AppCatalogEntry { pub id: String, pub title: String }
```

---

## Terminal apps — `RuosTermIo`

To embed a shell, use `RuosTermIo` (a zero-sized handle over the [`term`](term.md)
module) and pair it with `wm.wake_on_pty` (done for you by `term_open`):

```rust
use ruos_window::RuosTermIo;       // impls gui_core::platform::TermIo
// term_open() -> Option<handle>, term_read/write/resize/close(handle, …)
```
Drive `gui_core`'s terminal `DeskApp` with it, or call the raw [`term`](term.md) fns.
The handle is a PTY pair index; read is non-blocking (`-1` = the shell exited).

---

## Telemetry

For a System-Monitor-style app, the [`sys`](sys.md) module exposes CPU / process /
memory / uptime blobs. `gui-core` has a `sysinfo` DeskApp that parses them.

---

## What you do NOT do

- Don't open a window, run a loop, or call `commit` yourself — the compositor calls
  your `frame()`; `frame_once` commits.
- Don't draw a title bar or handle window dragging — `frame_once` does.
- Don't use `std::thread`, sockets, or files for UI — egui is immediate-mode; keep
  state in statics. (Tools that need fs/net are CLI `.wasm`, not GUI windows — see
  [ruos.md](ruos.md) / [wasi.md](wasi.md).)
