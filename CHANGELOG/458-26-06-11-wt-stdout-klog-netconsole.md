# 458 — stdout app finestra → anche klog ring + netconsole

**Data:** 2026-06-11

## Cosa

Nel ramo console di `fd_write` (shim WASI Wasmtime, `wt/wasi.rs`): oltre a
`console::CONSOLE` (serial+framebuffer), lo stdout/stderr di un'app finestra
ora viene anche spinto nel **ring dmesg** (`klog::push`) e, con la feature
attiva, su **netconsole** (`netconsole::enqueue`). Nota in `docs/api/wasi.md`.

## Perché

Debug del viewer su HW reale: i `println!` dell'app non comparivano da nessuna
parte ("seriale vuoto"). Causa: `CONSOLE` scrive solo serial+framebuffer — col
desktop attivo il framebuffer è coperto dalla GUI, e sul bare-metal senza
porta seriale il canale di log è netconsole, che riceve solo le righe passate
da `binfo!`/klog. Lo stdout guest non ci passava → strumentazione app
invisibile. Ora `dmesg` e `netconsole-rx` vedono anche i print delle app.

## File toccati

- kernel/src/wasm/wt/wasi.rs
- docs/api/wasi.md
