# Module `wm` — window manager

Window control + input for GUI apps. Runtime: **Wasmtime AOT** (`.cwasm`).
Source: `kernel/src/wasm/wt/wm.rs` (`func_wrap("wm", …)`).
Guest declarations: `ruos-desktop/crates/ruos-window/src/lib.rs` (`mod wm`).

Most apps use the `ruos-window` wrappers (`frame_once`, `WindowState`,
`declare_manifest!`) and never call these raw — reach here for `spawn`, taskbar /
launcher lists, drag, power.

**Last reviewed:** 2026-06-11 (24 functions; added `set_overlay()`,
`power_pending()`, `power_cancel()`; `poweroff()`/`reboot()` are now deferred + cancellable).

```rust
#[link(wasm_import_module = "wm")]
extern "C" { /* signatures below */ }
```

---

## Surface

### `commit(ptr: *const u8, len: u32, w: u32, h: u32)`
Copy the guest's `w×h` RGBA8888 surface (`len = w*h*4` bytes at `ptr`) into this
window's framebuffer and mark it committed. Call once per frame after rendering.

### `surface_size() -> i64`
Full screen framebuffer size, packed `(w << 32) | h`.

### `window_size() -> i64`
This window's kernel-assigned content size, packed `(w << 32) | h`. Use it to size
your egui surface (the kernel may resize/maximize the window).

---

## Identity & lifecycle

### `app_id() -> u32`
This window instance's unique id (stable for the window's life).

### `close()`
Request the compositor tear down this window. The process is despawned.

### `tick()`
Bump the call counter (used for spike instrumentation). Returns nothing.

### `minimize()`
Hide the window into the taskbar.

### `toggle_maximize()`
Toggle maximize (work-area) / restore.

### `activate(id: u32)`
Un-minimize + raise + focus the window with id `id` (e.g. from `window_list`).

### `set_background()`
Flag THIS window as the desktop background: z-bottom, undecorated, full-screen.
Used by the shell.

### `start_move()`
Begin a kernel-driven interactive drag (call when the title bar is grabbed; the
kernel follows the mouse until release).

### `stay_awake()`
Request a repaint on the next compositor loop (egui `request_repaint` equivalent) —
keeps animating without input.

---

## Input

### `poll_event(ptr: *mut u8)`
Drain ONE pending input event into a **20-byte** area at `ptr`, encoding
`option<gfx-event>`:

| Offset | Size | Field |
|--------|------|-------|
| 0 | 4 | `present` (0 = no event, 1 = event follows) |
| 4 | 4 | `kind` |
| 8 | 4 | `p0` |
| 12 | 4 | `p1` |
| 16 | 4 | `p2` |

`kind` (see `gui-core::abi::GfxEvent`):

| kind | meaning | p0 | p1 | p2 |
|------|---------|----|----|----|
| 0 | Key | scancode (PS/2 set 1) | pressed (0/1) | — |
| 1 | MouseMove | x (f32 bits) | y (f32 bits) | — |
| 2 | MouseButton | button (0=L,1=R,2=M) | pressed (0/1) | — |
| 3 | Resize | width | height | — |
| 4 | Quit | — | — | — |
| 5 | Wheel | detents (i32 two's complement; positive = scroll up / away from user) | — | — |

Call repeatedly until `present == 0` to drain the queue each frame. `ruos-window`
does this and feeds egui `RawInput`.

Wheel events are routed by the compositor to the **topmost window under the
cursor** (hover-scroll), preceded by a window-local MouseMove so egui's pointer
is positioned over the area to scroll. Sources: PS/2 IntelliMouse (4-byte
packets, enabled at boot when the device answers ID 3 — QEMU does) and USB HID
boot mouse byte 3.

### `wake_on_pty(idx: i32)`
Wake this window whenever PTY pair `idx` produces output (so a terminal window
repaints on async shell output). `idx < 0` unbinds. See [`term`](term.md).

---

## Spawning & lists

### `spawn(name_ptr: *const u8, name_len: u32)`
Launch `/bin/<name>.cwasm` (or `/mnt/apps/<name>.cwasm`) as a NEW window. `name` is
a UTF-8 string at `name_ptr`. `name` = the target app's `manifest()` id.

### `window_list(ptr: *mut u8, max: u32) -> u32`
Write up to `max` taskbar records at `ptr`; return the count written. Each record is
**32 bytes**: `u32 id`, `u32 flags`, then a 24-byte UTF-8 title (NUL-padded).

### `app_list(ptr: *mut u8, max: u32) -> u32`
Write up to `max` launcher-catalog records at `ptr`; return the count. Each record is
**64 bytes**: 24-byte id + 40-byte title (both UTF-8, NUL-padded). The catalog is the
compositor's `manifest()` scan of `/bin` + `/mnt/apps`.

---

## Time & power

### `wall_seconds() -> f64`
Monotonic seconds since boot. Use for `egui::RawInput.time` / animations (NOT a
wall clock).

### `poweroff()`
Request a deferred poweroff: returns immediately; the kernel powers off after
10 s unless cancelled (the compositor shows a countdown modal with a Cancel
button / Esc). Calling it again while a request is pending is a no-op.

### `reboot()`
Twin of `poweroff()` for restart: deferred 10 s, cancellable from the modal
(the shell's reboot button).

### `power_pending() -> i64`
`0` = no deferred request; else `(kind << 32) | ticks_remaining` (kind `1` =
poweroff, `2` = reboot; 100 ticks = 1 s). Source of truth for the countdown.

### `power_cancel()`
Cancel the pending deferred poweroff/reboot (no-op when none).

### `set_overlay()`
Flag THIS window as the notifications overlay: full-screen, composited ABOVE
all windows with per-pixel alpha, receives input only on pixels with
alpha ≥ 32 (plus ALL input while a power request is pending). One overlay max.

### `exit_to_shell()`
Tear the compositor down and hand the framebuffer back to the text console (the
shell's "back to console" button). All windows close; control returns to the
console shell (which keeps running on its own core). Re-running `compositor` from
that shell rebuilds a fresh desktop. The kernel defers the teardown to just after
the current frame.

---

## Launcher manifest (export, not import)

An app makes itself launchable by EXPORTING `manifest() -> i64`:

```rust
ruos_window::declare_manifest!("<id>", "<Title>", <w>, <h>);
```

The compositor scans `/bin` + `/mnt/apps` for `*.cwasm` exporting `manifest()`.
`<id>` MUST equal the `.cwasm` stem (the `spawn` key). The `i64` packs
`(ptr << 32) | len` of a `id\u{1f}title\u{1f}w\u{1f}h` string in linear memory.
