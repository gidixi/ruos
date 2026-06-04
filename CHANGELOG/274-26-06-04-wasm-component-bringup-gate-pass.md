# 274 — WASM Component Model bring-up gate PASSES (runs on bare metal)

**Data:** 2026-06-04

## Cosa
Provato **end-to-end** che il Component Model di wasmtime gira nel runtime
**no_std AOT** di ruos. Un component `ruos:bringup` (importa `system.log`/
`poweroff`, esporta `run`) viene deserializzato + istanziato + eseguito come
boot-check; la sua `run()` chiama `system.log("WT-COMPONENT-OK")` sull'host.

Output seriale del boot-check:
```
[component] WT-COMPONENT-OK
INFO wt   component bringup run=0
```
Nessun errore deserialize/instantiate/run, nessun mismatch dell'AOT settings
hash. Self-test esistenti invariati (`exec W^X`, `zero-init`, `gfx blit` tutti ok).
`make test-boot` → TEST_BOOT_PASS.

## Perché
Gate decisivo del piano WIT/Component-Model (spec
`docs/superpowers/specs/2026-06-04-kernel-wasm-wit-comms-design.md`, piano
`docs/superpowers/plans/2026-06-04-wasm-component-bringup.md`). "Compila" era già
provato; questo prova "**gira**" su bare metal → sblocca la migrazione del
desktop a interfacce WIT (Plan 2: egui-su-component; Plan 3: bottone poweroff).

## Come (pipeline completa)
`wit/ruos-bringup.wit` (sorgente unica) → guest `tools/wt-bringup` (wit-bindgen
0.57, no_std, bump alloc) → `wasm-tools component new` → `wt-precompile
--component` (`Engine::precompile_component`, stessa `Config` del kernel) → kernel
`run_component` (`Component::deserialize` + `component::Linker` +
`bindgen!` host impl). Feature wasmtime `component-model` abilitata (no_std
confermato sul toolchain pinned nightly-2026-05-26). `run_cwasm` e i ~50 tool
wasip1 invariati (path separato).

## Note tecniche (per i piani successivi)
- Host bindgen 45: `Bringup::add_to_linker::<_, HasSelf<_>>(&mut linker, |s| s)`,
  `Bringup::instantiate`, `bringup.call_run`, trait `ruos::bringup::system::Host`.
- Guest wit-bindgen 0.57: `default-features=false, features=["macros","realloc",
  "bitflags"]` (il default `std` collide con `#[panic_handler]`); trait generato
  `Guest`, `export!(Component)`, import `crate::ruos::bringup::system::log(&str)`.
- `wasm-tools component new` strippa gli import inutilizzati (il component importa
  solo `log`); il `.wit` dichiara comunque entrambi → l'host implementa entrambi.
- Toolchain: wasm-tools 1.251.0, wit-bindgen-cli 0.57.1.
- `.cwasm` sono build artifact gitignored (non committati); rigenerati dalla regola
  Makefile `kernel/src/wasm/wt/bringup.cwasm`.

## File toccati
- kernel/Cargo.toml (feature component-model), kernel/Cargo.lock
- wit/ruos-bringup.wit, tools/wt-bringup/* , tools/wt-precompile/src/main.rs + Cargo.toml
- kernel/src/wasm/wt/component.rs (nuovo), kernel/src/wasm/wt/mod.rs
- kernel/src/boot/phases/interrupts.rs (boot-check), Makefile (regola bringup.cwasm)
