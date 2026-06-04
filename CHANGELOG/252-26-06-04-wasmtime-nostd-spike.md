# 252 — Spike: Wasmtime no_std AOT — GO

**Data:** 2026-06-04

## Cosa
Spike di integrazione Wasmtime 45 no_std (AOT, no Cranelift on-device) in ruos:
- dep kernel `wasmtime = "=45.0.0"` features `runtime` + `custom-virtual-memory`;
- platform shim `kernel/src/wasm/wt/platform.rs`: TLS get/set + mmap/mprotect/
  munmap/remap/page_size cablati su frame allocator + paging, memory-image
  declinate (no CoW). Nessun setjmp/longjmp né signal (signals-based-traps off);
- `kernel/src/wasm/wt/mod.rs`: `run_hello` (deserialize → instantiate → call) con
  Config che combacia col compile;
- host precompiler `tools/wt-precompile` (genera `.cwasm` con settings identici);
- self-test boot-checks che esegue un `hello.cwasm` embeddato.

Esito **GO**: `wt hello print=42` / `wasmtime AOT hello ok`, boot completo
(`init.sh complete`). Codice nativo AOT eseguito in pagine W^X dentro ruos.

## Perché
Gate decisionale del desktop egui: l'AOT dà velocità quasi-nativa senza portare
Cranelift nel kernel. GO → si procede ai piani #4–#7. Dettagli e ricetta esatta
in docs/superpowers/plans/2026-06-04-wasmtime-nostd-spike.md (§Decision).

## File toccati
- kernel/Cargo.toml
- kernel/src/wasm/mod.rs
- kernel/src/wasm/wt/mod.rs
- kernel/src/wasm/wt/platform.rs
- kernel/src/wasm/wt/hello.cwasm (artefatto demo)
- kernel/src/boot/phases/interrupts.rs
- tools/wt-precompile/{Cargo.toml,src/main.rs}
- tools/wt-hello/hello.wat
