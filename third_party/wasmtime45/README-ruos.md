# README-ruos — wasmtime 45 (ruos fork)

## Provenienza

Tarball verbatim da crates.io: **wasmtime 45.0.0** (upstream repo sha `377cd917`,
path `crates/wasmtime` — vedi `.cargo_vcs_info.json`). Vendorizzato in
`third_party/wasmtime45/` da CHANGELOG/486.

## Chi lo usa

**Solo il kernel**, tramite `[patch.crates-io]` in `kernel/Cargo.toml`.
`tools/wt-precompile` usa intenzionalmente il wasmtime **STOCK** di crates.io
(workspace separato, `std`): non tocca mai i path patchati.

## Patch applicate (3 file, ogni hunk marcato `// ruos:`)

Per trovare tutti gli hunk: `grep -rn "ruos:" Cargo.toml src/`

1. **`Cargo.toml`** — feature `threads`: rimosso il dep su `"std"` per far
   compilare SharedMemory in `no_std`.
2. **`src/runtime/vm.rs`** — `mod parking_spot` gated a
   `all(feature = "threads", feature = "std")`.
3. **`src/runtime/vm/memory/shared_memory.rs`** — import `std` → `alloc`/`core`;
   `std::sync::RwLock` → sync layer interno (`custom-sync-primitives`); blocco
   `memory.atomic.wait32/wait64/notify` deviato su tre hook `extern "C"` del
   kernel: `wasmtime_futex_wait32`, `wasmtime_futex_wait64`, `wasmtime_futex_notify`.

## Re-vendor per wasmtime 46+

1. Estrarre il tarball pristine da crates.io.
2. Riapplicare i 3 hunk sopra.
3. Ricompilare: `make iso`.
4. Re-AOT di sistema: fatto in automatico da `make iso`.
5. Re-AOT esterno (`apps/` drop-folder e `/mnt/apps`): necessario **solo se
   cambiano le tunables** (vedere `docs/api/README.md §.cwasm compatibility`).

## Riferimenti

- Spec MT Fase 2: `docs/superpowers/specs/2026-06-12-wasm-mt-fase2-threads-design.md`
- Changelog: `CHANGELOG/486-26-06-12-wasmtime-fork-threads-nostd.md`
