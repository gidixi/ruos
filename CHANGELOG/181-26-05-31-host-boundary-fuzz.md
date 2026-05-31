# 181 — Host-boundary fuzz tests + fuel-kill integration + README security model

**Data:** 2026-05-31

## Cosa

### Part A — Host-runnable adversarial bound-check tests
Added a standalone host-native crate `tools/boundcheck-test/` that mirrors the
pure `check_bounds` function from `kernel/src/wasm/host/mem.rs` (no wasmi, no
no_std, no custom target). The crate is a normal `std` binary that runs five
adversarial test cases:

- `negative_ptr_or_len_rejected` — negative ptr/len (including i32::MIN) → EINVAL (28)
- `past_end_rejected` — past-end access and MAX+MAX overflow → EFAULT (21)
- `zero_len_ok_at_boundary` — zero-len at exact end and zero-size → Ok
- `in_range_ok` — full-range and mid-range → Ok
- `never_panics_exhaustive_small` — exhaustive 205×205×200 grid (8 405 000 calls), function must never panic

All five pass: `cargo run` in `tools/boundcheck-test/` (WSL, stable toolchain).

A SYNC RULE comment in the test file requires the copy to be updated if
`check_bounds` changes in the kernel.

### Part B — Integration: tight-loop .wasm fuel-killed, kernel survives
Added `user/spinloop/` — a minimal `wasm32-wasi` binary whose `main` spins in
an unbounded compute loop with NO host calls. The wasmi fuel budget
(2 000 000 000 instructions) is exhausted without any refuel event; the kernel
terminates the instance and logs `wasm: task killed (fuel exhausted)`.

Wired into the build system:
- `user/Cargo.toml` workspace member `spinloop`
- `Makefile` `BIN_TOOLS` + `user-bin/%.wasm` pattern rule
- `limine.conf` module entry so the VFS sees `/bin/spinloop.wasm` at boot

Added `tests/fuel-test.sh`: boots ruos headless, runs `/bin/spinloop.wasm`
over SSH, then issues a second SSH command (`ls /bin | wc -l`) to prove the
kernel kept serving. Asserts both:
1. `wasm: task killed (fuel exhausted)` in serial.log
2. A numeric count in the survival response

Added `Makefile` target `run-fuel-test`. Result: `TEST_PASS_FUEL`.

### Part C — README security model section
Added `## Security model` to `README.md` covering:
- Everything runs in ring 0; WASM runtime is the sandbox, not CPU rings.
- Hardening table: host-boundary safety (fuzz-tested), fuel metering, per-task
  resource limits, capability-scoped paths, non-deadlocking panic + reset.
- Honest ceiling: defence-in-depth in ring 0; a wasmi memory-safety bug or
  kernel `unsafe` bug is fatal; only separate address space + CPU privilege
  could contain it (explicitly out of scope).

## Perché

Task 6 (final task) of the blast-radius hardening feature:
- Prove the host-boundary check is safe with host-runnable adversarial tests.
- Prove fuel metering kills runaway compute and the kernel survives.
- Document the honest security ceiling so users don't over-claim.

## File toccati
- tools/boundcheck-test/Cargo.toml (new)
- tools/boundcheck-test/src/main.rs (new)
- user/spinloop/Cargo.toml (new)
- user/spinloop/src/main.rs (new)
- user/Cargo.toml
- user-bin/spinloop.wasm (new, built artefact)
- limine.conf
- Makefile
- tests/fuel-test.sh (new)
- README.md
- CHANGELOG/181-26-05-31-host-boundary-fuzz.md (this file)
