# egui apps in the compositor — SP-B: egui-reactor harness + Client-Side Decorations (design)

**Date:** 2026-06-05
**Status:** approved (brainstorm), pending spec review → writing-plans
**Branch:** `feat/egui-compositor-sp-b`

## Context — the 3-sub-project arc

Goal: **run real egui apps as windows of the kernel-side compositor.** SP-A (DONE,
merged `e51d789`, CHANGELOG 289/290) unified the store state so a `wasm32-wasip1`
(std) guest runs as a compositor window (`Linker<AppState>` = WASI + `wm`), proven by
the solid-colour `wasip1-probe`. SP-B makes a real **egui** app render in a window.
SP-C (later) builds the system-info app on SP-B's harness.

**Decision (user, 2026-06-05): Client-Side Decorations (CSD).** The egui app draws the
WHOLE window — title bar, title text, `[X]`, and content. The kernel's server-side
`decor` module is dropped. The kernel becomes a pure compositor (composite raw
surfaces + route input + window management host fns). Rationale: egui-native, coherent
look; simpler kernel; the title bar widget is written once in egui and reused by every
app. Accepted trade-offs: the demo solid-colour reactors lose their kernel chrome (they
are retired from the launcher, kept for boot-checks); a broken app's `[X]` could be
unreachable, mitigated by `proc_exit`/trap → auto-reap + a taskbar window-list close.

## Goal (SP-B)

A minimal **egui** app (a window titled "egui demo" with a CSD title bar + a label +
a counter button) spawns from the launcher and composites as a window: its title bar,
text, `[X]` button and content are all egui-rendered; dragging the title bar moves the
window; clicking `[X]` closes it; clicking another window focuses it. `gui-core` (the
egui raster/input pipeline) is reused **unchanged**.

## Architecture

Two coupled parts (coupled because the CSD compositor path can only be verified by an
app that draws its own chrome + drives `wm.move`/`wm.close`):

### Part 1 — Compositor CSD migration (`kernel/src/wasm/wt/wm.rs`)

- **Drop server-side decorations.** Remove the `decor` module's drawing (title bar,
  title text, `[X]`) and `decor::hit`. `compose_window(idx)` returns the window's **raw
  surface** (`store.data().win.pixels`) at `Window.rect` — no title-bar band added.
  `Window.rect` is the WHOLE window (the surface IS the whole window). Decorations are
  now the app's pixels.
- **New host fn `wm.move(dx, dy)`** (signed i32): translate `Window.rect` by `(dx,dy)`,
  clamped on-screen. The app calls it while dragging its title bar. (Relative delta, not
  absolute set-pos, so the app needn't know its screen origin.)
- **Input dispatch simplified.** On a left-button-down inside a window: kernel does
  `raise` + `set_focus` (unchanged), then forwards the event (window-local coords) to
  the focused window's queue. There is no more `decor::hit` Close/Title branch — the app
  decides what a click means (its `[X]`, its title bar, its content). The kernel keeps
  routing all subsequent events to the focused window.
- **Crash safety-net: `proc_exit`/trap → reap.** Map a guest `proc_exit` (today
  `wasi.rs` traps-to-unwind) so that, for a window store, it sets `win.close_requested`
  instead of poisoning the instance; and make `frame_all` treat a `frame()` that returns
  `Err` (trap/panic-abort) as `close_requested = true`. The existing reap pass then
  drops the window cleanly. (Today `frame_all` silently swallows the `Err` → a crashed
  window goes black but stays; CSD makes that unacceptable since its `[X]` is gone.)
- **Retire the demo reactors from the launcher.** Add `show_in_launcher: bool` to
  `AppEntry`. The solid reactors (`react-A`, `react-B`, `selfclose`, `wasip1-probe`) get
  `false` (still spawnable by name for boot-checks); the egui demo gets `true`.
  `draw_launcher`/`launcher_hit` iterate only `show_in_launcher` entries. Boot-checks
  call `spawn_app` by registry name, unaffected.
- **Safety-net (minimal): taskbar window-list close.** The launcher taskbar also lists
  OPEN windows; a click on a window entry's close affordance calls `wm.close(id)`
  kernel-side, independent of the app's CSD `[X]`. (Minimal version in SP-B; a richer
  window-manager taskbar is later.)

### Part 2 — egui-reactor harness (`ruos-desktop/` submodule)

- **New crate** (e.g. `ruos-desktop/compositor-app/`) in the workspace, depending on
  `gui-core` (the portable egui pipeline: `raster.rs`, `input.rs`, `platform.rs`,
  `Gui`). It is a **`wasm32-wasip1` reactor** (`crate-type=["cdylib"]`, no `main`,
  exports `frame()`), reusing SP-A's proven wasip1-reactor shape + `run_initialize`.
- **A `Platform` impl over `wm`** (`gui-core`'s single IO seam): `present(buf,x,y,w,h)`
  → `wm.commit(buf, len, w, h)`; `poll_events()` → drain `wm.poll_event` (the same
  20-byte `GfxEvt` layout `gui-core/src/input.rs` already decodes); `surface_info()` →
  a fixed `w×h` (480×320 v1) + format RGBA8888; `wall_clock_secs()` → a new
  `wm.wall_seconds()` host fn (the compositor linker lacks one; wrap the kernel's
  existing monotonic `gfx::wall_secs()` source); `poweroff()` → unused/stub.
- **Reusable CSD title-bar widget** (egui): draws a top bar (title text) + an `[X]`
  button; returns interaction intents. On `[X]` clicked → call `wm.close()`; on a drag
  starting in the bar → accumulate the pointer delta and call `wm.move(dx,dy)`. Lives in
  the new crate so every egui app reuses it.
- **The demo app:** a tiny egui `App` — the CSD title bar ("egui demo") + a `CentralPanel`
  with a label and a counter button (`if ui.button("clicked {n}").clicked() { n += 1 }`)
  — proving real egui widgets + input + state in a window.
- **Frame loop inversion:** the exported `frame()` does ONE egui pass: `poll_events()` →
  feed `gui-core` input → `Gui::frame()` (egui `ctx.run` + tessellate + raster into the
  persistent `tiny_skia::Pixmap`) → `present()` (= `wm.commit`). No internal loop, no CPU
  ownership — the compositor's `frame_all` drives it.

## Data flow (one frame of an egui window)

```
compositor frame_all() → app.frame():
  1. wm.poll_event* → Vec<GfxEvt>          (drain this window's queue)
  2. gui-core input.rs: GfxEvt → egui::RawInput  (pointer/clicks/keys/modifiers)
  3. egui ctx.run(|ui| { titlebar(); content(); })
       - titlebar [X] clicked → wm.close()
       - titlebar drag        → wm.move(dx,dy)
  4. gui-core raster.rs: tessellate + raster → Pixmap (480×320 RGBA8888, dirty-rect)
  5. wm.commit(pixmap, len, 480, 320)
compositor: compose_window = raw surface; present() (SMP) → blit
```

## Error handling

- **`proc_exit` in a window reactor** → `win.close_requested = true` (graceful reap),
  not a trap-unwind that poisons the instance.
- **`frame()` returns `Err` (trap / panic=abort)** → `frame_all` sets
  `close_requested = true` → reaped next loop. The window disappears cleanly instead of
  freezing black with no reachable `[X]`.
- **Hung `frame()` (infinite loop, never returns):** out of scope — the cooperative
  single-core model means any hung `frame()` freezes the whole compositor regardless of
  CSD/SSD; the mitigation is that egui is immediate-mode (`frame()` always returns).
- **Instantiate failure** (missing import): already `None` from `spawn_app` + id freed.

## Testing / verification

1. **Build** — kernel (3 profiles) + the new wasip1 egui crate → `.cwasm`. The command-
   app path + existing boot-checks still pass.
2. **Boot-check (headless)** — spawn the egui demo by name against `Linker<AppState>`,
   call `frame()` once, assert `win.pixels` non-empty (the egui raster committed).
   Marker `egui demo spawn ok pixels=<480*320*4>`. (egui's first frame must render +
   commit.)
3. **Visual (QEMU+KVM QMP)** — launch "egui demo" from the taskbar → a window with an
   **egui-drawn title bar + title text + [X] + a label + a counter button**; click the
   counter (it increments — proves egui input + state); **drag the title bar** (window
   moves — proves `wm.move`); click **[X]** (window closes — proves CSD close); click
   another window (focus changes). Re-verify text rendering in-window (the SSE4.1/ROUNDSS
   glyph fix is global, should hold).
4. **VBox** (VM `ruos`, `[[vbox-test-harness]]`) — the egui window renders on HW-like.

## Risks

- **Per-window memory:** an egui window ≈ 4 MB linear memory + font atlas + persistent
  `tiny_skia` canvas. At `MAX_WINDOWS=8` watch the kernel heap (128 MiB now per CLAUDE.md
  step 4 — comfortable, but monitor).
- **Per-frame cost:** the compositor `run()` busy-spins calling `frame()` on every window
  each iteration; egui tessellate+raster per window may be heavy. `gui-core`'s dirty-rect
  makes a no-change frame cheap, but if responsiveness/SSH suffers, add repaint-on-demand
  (only call `frame()` when egui `requested_repaint` or input arrived) — flagged, deferred.
- **`wm.move` drag math:** the app must compute the pointer delta correctly (window-local
  coords + the grab offset) so the window tracks the cursor without jumping. Mirror SP3's
  drag-grab logic, now app-side.
- **Input gaps egui wants:** no scroll-wheel kind in the wire format; only LEFT modifier
  variants; Resize/Quit ignored by `gui-core` input.rs. The counter demo doesn't need
  them; scroll/clipboard come later if an app needs them.
- **Retiring reactors changes boot-check counts:** `launcher registry apps=4` will change;
  update the boot-check assertions to the new launcher-visible count (or assert on the
  egui demo specifically).
- **Submodule edits:** SP-B adds a crate to `ruos-desktop` (the submodule). Coordinate the
  submodule commit + the kernel's `include_bytes!` of the new `.cwasm`.

## Out of scope (SP-B)

- Window resize, scroll, clipboard, IME, multiple simultaneous egui apps.
- A full window-manager taskbar (minimal window-list close only).
- WIT-ification of the `wm`/surface protocol (raw `wm` extern for now).
- The system-info app + its kernel data channel (SP-C).
- Repaint-on-demand scheduling (only if cost forces it).

## Provides (for SP-C)

- The egui-reactor harness (`compositor-app` crate: `Platform`-over-`wm` + `frame()`
  export + `run_initialize`) — SP-C's system-info app is another egui `App` in the same
  crate/shape.
- The reusable CSD title-bar widget — SP-C reuses it.
- `wm.move`/`wm.close` + the CSD input model — SP-C inherits them.
- The kernel data-channel need (sysinfo reads `proc::list`/cpu/mem, which a sandboxed
  guest cannot) is the new piece SP-C adds (a `wm.sysinfo`-style host fn).
