# Module `term` — terminal bridge

Attach a GUI window to a real kernel PTY pair + shell (the Terminal app). Runtime:
**Wasmtime AOT** (`.cwasm`).
Source: `kernel/src/wasm/wt/term.rs` (`func_wrap("term", …)`).
Guest declarations: `ruos-desktop/crates/ruos-window/src/lib.rs` (`mod term`,
`RuosTermIo`).

The handle returned by `open` is a PTY **pair index** (`0..NUM_PAIRS`). Pair 0 is
the framebuffer console; `term.open` claims a free pair `1..NUM_PAIRS` (8 pairs,
i.e. 7 usable, shared with SSH). Pair it with [`wm.wake_on_pty`](wm.md) so the
window repaints when the shell emits output asynchronously.

**Last reviewed:** 2026-06-09 (5 functions).

```rust
#[link(wasm_import_module = "term")]
extern "C" { /* signatures below */ }
```

---

### `open() -> i32`
Claim a free PTY pair and spawn `/bin/shell.wasm` on its slave. Returns the handle
(pair idx) or **`-1`** if all pairs are busy. Typically followed by
`wm.wake_on_pty(handle)`.

### `read(h: i32, ptr: *mut u8, cap: u32) -> i32`
Drain up to `cap` bytes of shell output into `ptr`, **non-blocking**. Returns:
- `> 0` — bytes written,
- `0` — nothing ready,
- `-1` — EOF (the shell exited AND its output is fully drained → close the window).

### `write(h: i32, ptr: *const u8, len: u32)`
Push `len` bytes (user keystrokes) at `ptr` to the shell's input through the line
discipline (cooked mode, `^C`, echo).

### `resize(h: i32, cols: u32, rows: u32)`
Set the PTY window size (character cells) so the shell/app can lay out.

### `close(h: i32)`
SIGHUP the shell and release the pair. (`RuosTermIo` also unbinds
`wm.wake_on_pty(-1)`.)
