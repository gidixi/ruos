# Module `ruos_gfx` — raw framebuffer graphics

Raw framebuffer host functions for GUI apps. This module is the Wasmtime AOT counterpart to the `ruos:gui/gfx` component interface, used by GUI applications to bypass the window manager and draw directly to the screen.
Runtime: **Wasmtime AOT** (`.cwasm`).
Source: `kernel/src/wasm/wt/gfx.rs` (`func_wrap("ruos_gfx", …)`).
Guest declarations: Used internally by `ruos-desktop/gui-core`.

**Last reviewed:** 2026-06-09.

```rust
#[link(wasm_import_module = "ruos_gfx")]
extern "C" { /* signatures below */ }
```

---

### `gfx_info(out_ptr: *mut u8) -> i32`
Writes 16 bytes (4×`u32` little-endian: `width, height, stride, format`) describing the framebuffer surface. Calling this marks the guest as a GUI app, entering GUI mode (and silencing console output). Returns `0` on success, `28` on fault.

### `gfx_blit(buf_ptr: *const u8, buf_len: u32, x: u32, y: u32, w: u32, h: u32) -> i32`
Blit an RGBA8888 surface to the screen at the given `(x, y)` coordinates. Returns `0` on success, `28` on fault.

### `gfx_poll_event(out_ptr: *mut u8, max: u32, timeout_ms: i32) -> i32`
Poll up to `max` pending input events into `out_ptr`. Each event is 16 bytes: `kind`, `p0`, `p1`, `p2` (all `u32` LE). Returns the number of events actually written. (Timeout is currently unused and returns immediately).

### `gfx_pending() -> i32`
Returns the number of queued GUI events. This lets the app skip rendering when nothing has changed.

### `gfx_debug(ptr: *const u8, len: u32)`
Log a UTF-8 string directly to the kernel serial port. This bypasses the PTY, which isn't drained while a synchronous GUI owns the executor.

### `gfx_wall_secs() -> f64`
Returns monotonic wall-clock seconds (with 10 ms fraction). Safe for UI animations (e.g. `egui::RawInput.time`), as it never goes backward.
