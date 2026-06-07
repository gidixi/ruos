# 296 — egui SP-C window-SDK + kernel mechanism (verified)

**Data:** 2026-06-05

## Cosa

SP-C (compositor window SDK + kernel spawn/bg mechanism) complete and verified.

**Kernel changes (wm.rs + supporting files):**

- `wm.spawn(name)`: deferred spawn request — guest calls `wm.spawn("egui-demo")`; the
  run loop resolves `/bin/<name>.cwasm` via the VFS after `frame_all` (no mid-iteration
  mutation of `wins`) and calls `spawn_named`. Request field changed from
  `Option<String>` to `VecDeque<String>` (this fix, CHANGELOG 296a) so multiple
  `wm.spawn` calls in one frame are all honoured.
- `wm.set_background()`: deferred bg-request — calling window is pinned as the
  full-screen, z-bottom, undecorated background; receives input only where no
  non-`bg` window covers the point (input fallthrough).
- `spawn_named`: the ONE instance-creation path (embedded `APPS` + VFS by-name both
  route here); allocates window-id, instantiates a fresh `Store<AppState>`, runs
  `_initialize`, cascades placement, raises+focuses, registers a proc.
- Kernel launcher draw/hit code dropped (WM shrink); desktop UX (panel/launcher/
  wallpaper) moved to userspace SP-D shell which `wm.spawn`s apps + calls
  `wm.set_background()` on itself.
- Heap 128 → 256 MiB: each egui instance reserves 48 MiB linear memory; two windows
  needed more headroom.
- `limine.conf`: mounts `egui-demo.cwasm` into the VFS at `/bin/egui-demo.cwasm` so
  the VFS `wm.spawn` path is exercised at boot (initial window loaded from VFS,
  fallback to embedded blob).

**ruos-window SDK (new crate, `ruos-desktop/ruos-window`):**

- `frame_once` reactor harness (calls `frame()` once per kernel tick).
- Titlebar draw + [X] / drag-sense helpers (CSD; kernel draws nothing).
- `wm` bindings: `commit`, `app_id`, `close`, `start_move`, `spawn`, `set_background`,
  `wall_seconds`, `poll_event` — extracted from compositor-app into a reusable lib.

**compositor-app refactor:**

- Thinned to use the `ruos-window` SDK; adds "spawn another" + "make bg" test buttons
  that exercise the two new mechanisms from userspace.

**VecDeque fix (this entry):**

- `WmState.spawn_request` changed from `Option<String>` (last-wins) to
  `VecDeque<String>` (FIFO queue). All four WmState literals updated to
  `VecDeque::new()`, host fn changed to `.push_back()`, run-loop drain changed from
  `if let Some ... .take()` to `while let Some ... .pop_front()`, and
  `spc_self_test` updated identically.

**Verified:**

- Boot-check: `wm spc flags=0b11` + `egui demo spawn ok pixels=614400` present in
  `build/test-boot.log`.
- QEMU: `wm.spawn ok` — second egui window appears; "make bg" → bg window full-screen.
- VBox: clean boot, no regressions.

## Perché

SP-C spec (`docs/superpowers/specs/2026-06-05-egui-compositor-sp-c-spec.md`) requires
a kernel `wm.spawn` deferred-spawn mechanism + `wm.set_background()` bg-window support,
a reusable SDK lib, and a compositor-app that exercises both from userspace. The
`Option<String>` spawn_request was a latent spec violation (concurrent spawns silently
dropped); fixed to `VecDeque<String>` per spec.

## File toccati

- kernel/src/wasm/wt/wm.rs
- kernel/src/wasm/wt/mod.rs
- kernel/src/boot/phases/interrupts.rs
- kernel/src/memory/heap.rs
- limine.conf
- ruos-desktop/ruos-window/src/lib.rs (new crate)
- ruos-desktop/compositor-app/src/main.rs
- build/spc_verify.py
