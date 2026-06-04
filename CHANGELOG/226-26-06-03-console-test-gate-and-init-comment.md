# 226 — Gate engine self-test behind boot-checks; init script comment

**Data:** 2026-06-03

## Cosa
- `kernel/src/boot/phases/devices.rs`: wrapped the `engine_test::run()` call in
  `#[cfg(feature = "boot-checks")]`, matching the identical gate already on the
  `fb::self_test` call above it. The test function no longer runs on every
  production boot (QEMU interactive, VBox, real hardware, SSD-installed systems).
- `Makefile`: added `CARGO_FEATURES ?=` variable forwarded to the `build` target
  via `$(if $(CARGO_FEATURES),--features $(CARGO_FEATURES),)`. Updated the
  `run-console-test` target to pass `CARGO_FEATURES=boot-checks` when rebuilding
  the ISO, so `engine_test::run()` is compiled in and the `CONSOLE_TEST: OK`
  marker is still emitted during the test run.
- `user-bin/console-test-init.sh`: prepended a `#` header comment explaining the
  purpose of the script, matching the house style used by `smoke.sh`, `dm-init.sh`
  and other init scripts.

## Perché
Code review flagged two issues on the harness commit (225):
1. `engine_test::run()` was called unconditionally — it will grow heavier over
   time (grid/surface allocation, full-redraw perf loop) and must not run on
   production boots, exactly like the existing `fb::self_test` gate.
2. The init script lacked the standard header comment present on all other init
   scripts in `user-bin/`.

## File toccati
- kernel/src/boot/phases/devices.rs
- Makefile
- user-bin/console-test-init.sh
