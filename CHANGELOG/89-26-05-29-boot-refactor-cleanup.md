# 89 — Boot log migration + `make test-boot` target

**Data:** 2026-05-29

## Cosa

- Migrated all remaining init-time `kprintln!("ruos: ...")` calls to structured
  `binfo!` / `bwarn!` calls in submodule init functions called from boot phases.
- Added `make test-boot` Makefile target: builds kernel with `--features boot-checks`,
  assembles ISO, runs QEMU headless, asserts smoke lines + shell sentinel.

### Files changed

**`kernel/src/modules.rs`**
- Removed `use crate::kprintln` import.
- `mount_all()`: migrated 4 prints → `bwarn!("mod", ...)` / `binfo!("mod", ...)`.

**`kernel/src/net/mod.rs`**
- Removed `use crate::kprintln` import.
- Removed duplicate `kprintln!("ruos: net init ok addr=127.0.0.1/8")` — superseded
  by `binfo!("user", "net init 127.0.0.1/8 (loopback)")` in `phases/userland.rs`.

**`kernel/src/executor/mod.rs`**
- `run()`: migrated "executor: spawning tasks" and "executor: all tasks spawned"
  → `binfo!("user", ...)`.
- Left as `kprintln!`: `exec_worker` error paths (runtime), `tick_task` heartbeat
  (runtime async task).

**`kernel/src/timer.rs`**
- Removed `kprintln` import.
- `init()`: migrated "lapic calibrated N ticks/sec, periodic count=N"
  → `binfo!("irq", ...)`.

**`Makefile`**
- Added `test-boot` phony target: builds kernel with `--features boot-checks`,
  assembles ISO inline (avoids recursive make dependency chain), runs QEMU
  headless, greps for `smoke` and `shell: init.sh complete`, echoes `TEST_BOOT_PASS`.

### Intentionally NOT migrated

- `idt.rs` exception handlers (#DE, #UD, #DF, #GP, #PF, bp) — runtime interrupt
  handlers, not init-time.
- `executor/mod.rs:100,106` — `exec_worker` runtime error paths.
- `executor/mod.rs:141,145` — `tick_task` async runtime heartbeat.
- `main.rs:66` — pre-banner emergency path (before logger is usable).

## Perché

Completes Task 3 of the boot-refactor branch: all one-shot init logs now emit
as structured `[T+SECS.MILLIs] I  module  message` entries, giving a consistent
and grep-friendly boot trace. The `test-boot` target provides a repeatable CI
gate for boot-checks (smoke self-tests).

## File toccati

- kernel/src/modules.rs
- kernel/src/net/mod.rs
- kernel/src/executor/mod.rs
- kernel/src/timer.rs
- Makefile
- CHANGELOG/89-26-05-29-boot-refactor-cleanup.md
