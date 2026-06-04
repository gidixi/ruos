# 256 — Wasmtime WASI core: run real wasip1 binary

**Data:** 2026-06-04

## Cosa
Implementato il cuore del runtime Wasmtime (piano #7):
- `wasm/wt/state.rs` (`WtState`: argv + exit), `wasm/wt/mem.rs` (accessor memoria
  guest bounds-checked), `wasm/wt/wasi.rs` (WASI Preview 1 core: proc_exit,
  fd_write→CONSOLE, args_sizes_get/args_get, environ_sizes_get/environ_get);
- `wasm/wt/mod.rs`: engine condiviso (`engine()`) + `run_cwasm(cwasm, args)` che
  esegue `_start`.
- Boot-check: esegue `echo.cwasm` (vero `wasm32-wasip1` da user-bin/echo.wasm,
  precompilato con tools/wt-precompile) con argv ["echo","WT-ECHO-OK"].

Esito verificato in QEMU: `WT-ECHO-OK` su seriale + `wasmtime WASI echo exit=0`.
Un binario std reale gira via Wasmtime no_std + WASI Linker (non solo il demo
hand-rolled `ruos.print`).

## Perché
Foundation del desktop egui: la GUI app è un binario wasip1 std → stesso path.
Resto di #7 (VFS/path/fd_read via block_on, epoch blocking, router .cwasm nel
shell, pipeline Makefile) come follow-up.

## File toccati
- kernel/src/wasm/wt/{state.rs,mem.rs,wasi.rs,mod.rs}
- kernel/src/wasm/wt/echo.cwasm (artefatto test)
- kernel/src/boot/phases/interrupts.rs
