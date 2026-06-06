# 312 — SMP Step 5: GUI pinned to a dedicated core; BSP stays alive for I/O

**Data:** 2026-06-06

## Cosa

- `kernel/src/cpu/mod.rs`: added `CoreRole` enum (`BspIo`, `GuiCompositor`,
  `ComputeApp`) + `CORE_ROLES[MAX_CPUS]` atomic table + `set_core_role` /
  `core_role` accessors. Default for all cores = `ComputeApp`.
- `kernel/src/smp/mod.rs`: `bringup()` now pins BSP (cpu 0) to `BspIo` and
  the first AP (cpu 1) to `GuiCompositor` before calling `cpu.bootstrap`, so
  `ap_entry` reads the correct role instantly. Logs the role table after
  bringup completes.
- `kernel/src/cpu/ap.rs`: `ap_entry` dispatches on `core_role(cpu_id)`:
  `GuiCompositor` → `wm::gui_worker_loop()`; all others → `executor::run_core(cpu)`.
- `kernel/src/wasm/wt/wm.rs`: added `CompositorMailbox` (atomic ptr/len/ready),
  `gui_worker_loop()` (halts on mailbox until hand-off IPI, then calls
  `run_compositor_gate` forever), and `send_compositor_to_gui_core(bytes)` (publishes
  mailbox + wakes GUI core via `executor::wake_core`).
- `kernel/src/executor/mod.rs`: `exec_worker_task` compositor branch: leaks the
  bytes, calls `send_compositor_to_gui_core`; if handed off → completes the
  EXEC_QUEUE handshake (result/done/waker) + `continue` so the BSP executor keeps
  polling I/O; if no GUI core (1 CPU) → runs `run_compositor_gate` inline (fallback).
- `kernel/src/gfx/mod.rs`: removed the `crate::usb::poll()` band-aid from
  `fold_mouse`. The BSP executor (alive after hand-off) owns USB via `usb_poll_task`.
- `tests/ssh-during-gui-test.sh`: new GOAL GATE test — builds ISO with
  `compositor-init.sh`, boots with `-smp 4`, waits for the hand-off marker in
  serial, then SSHes; asserts `auth ok` + `ruos:/$` prompt.
- `Makefile`: added `.PHONY: run-ssh-gui-test` target (builds compositor ISO +
  runs the goal gate test).

## Perché

The compositor previously ran inline on the BSP executor (`exec_worker_task` →
`run_compositor_gate` → never returns), permanently blocking `ssh_serve_task`,
`net_poll_task`, and `usb_poll_task`. Step 5 pins the compositor to a dedicated
AP (cpu 1 = `GuiCompositor`), freeing the BSP executor to poll I/O in parallel.
Result: GUI fluid AND SSH/net/USB responsive simultaneously — THE GOAL.

## File toccati

- kernel/src/cpu/mod.rs
- kernel/src/smp/mod.rs
- kernel/src/cpu/ap.rs
- kernel/src/wasm/wt/wm.rs
- kernel/src/executor/mod.rs
- kernel/src/gfx/mod.rs
- tests/ssh-during-gui-test.sh
- Makefile
