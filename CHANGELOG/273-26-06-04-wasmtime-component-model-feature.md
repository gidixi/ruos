# 273 — Enable wasmtime component-model feature (no_std)

**Data:** 2026-06-04

## Cosa
Added `"component-model"` to the wasmtime dependency features in `kernel/Cargo.toml`.
Verified the kernel still builds no_std for `x86_64-unknown-none` under the pinned
nightly toolchain (`nightly-2026-05-26`) with only pre-existing dead-code warnings —
no new errors or regressions.

## Perché
Task 1 of the WASM Component Model bring-up plan: feasibility gate to confirm that
wasmtime's `component-model` feature is no_std-clean on the project's pinned toolchain
before any source-level work begins.

## File toccati
- kernel/Cargo.toml
