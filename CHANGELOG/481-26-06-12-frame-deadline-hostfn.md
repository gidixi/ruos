# 481 — `wm.frame_deadline_set/reset` host functions

**Date:** 2026-06-12

## What

Two new `func_wrap("wm", …)` host functions in `kernel/src/wasm/wt/wm.rs`:

- `frame_deadline_set(ticks: u32)` — re-arm the **calling window's** epoch
  deadline to `ticks` for the rest of the current frame.
- `frame_deadline_reset()` — restore it to `FRAME_DEADLINE_TICKS`.

Both call `caller.as_context_mut().set_epoch_deadline(...)` on the calling
store. Added `AsContextMut` to the `wasmtime` import.

## Why

The viewer browser app is gaining an in-app JavaScript engine (QuickJS via
rquickjs, linked into `viewer.cwasm`). A page's JS bootstrap can run far longer
than the regular per-frame watchdog deadline (`FRAME_DEADLINE_TICKS`), so the
viewer needs to widen its own deadline during synchronous JS `eval`, then drop
back to the regime for steady-state job pumping. The kernel already arms the
epoch deadline per-store before `frame.call` (`frame_all`); these functions let
the guest re-arm its own deadline mid-frame.

## Properties

- **Per-store / reentrancy-clean.** Each call touches only the calling window's
  store epoch — no shared/global kernel state. Safe under the planned parallel
  compositor (MT roadmap Fase 1): satisfies the "per-window state in the store,
  never in implicit globals" audit rule.
- **Non-breaking import set.** Existing apps that never import these functions
  are unaffected; no re-AOT of other `.cwasm` is forced. (`viewer.cwasm`
  re-AOTs on its next build as usual.)
- **Self-limiting.** `frame_all` re-arms the normal deadline before every frame,
  so an unbalanced `set` without `reset` only affects the current frame; it
  cannot permanently disable the watchdog for a window.

## App-side

The viewer declares the imports in `apps/viewer/src/js/host.rs`
(`with_extended_deadline(ticks, ||{…})`). Manual: `api/wm.md` (in the
ruos-test SDK repo), bumped to 26 functions / Last reviewed 2026-06-12.
