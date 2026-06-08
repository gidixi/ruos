# 339 — Host module `sys` per le finestre del compositor

**Data:** 2026-06-08

## Cosa
Nuovo modulo host Wasmtime `kernel/src/wasm/wt/sys.rs` (`sys.cpustat`/`proc_stat`/
`meminfo`/`uptime`), registrato sui linker delle finestre in `wm.rs`. Espone alla
GUI gli stessi blob che rtop legge via il modulo wasmi `ruos`.

## Perché
Il System Monitor (finestra egui su Wasmtime) deve leggere dati reali; gli host fn
`ruos` esistenti stanno solo sul linker wasmi.

## File toccati
- kernel/src/wasm/wt/sys.rs
- kernel/src/wasm/wt/mod.rs
- kernel/src/wasm/wt/wm.rs
- CHANGELOG/339-26-06-08-wt-sys-host-module.md
