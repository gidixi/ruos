# 486 — Fork wasmtime 45 vendored: feature `threads` in no_std (MT Fase 2 Task 0)

**Data:** 2026-06-12

## Cosa

Vendorizzato wasmtime 45.0.0 (sorgente registry, self-contained) in
`third_party/wasmtime45/` e applicata una patch minimale (ogni hunk commentato
`// ruos:`) per far compilare la feature `threads` senza `std`:

- `Cargo.toml`: la feature `threads` non trascina più `std`.
- `src/runtime/vm/memory/shared_memory.rs`: import std → `alloc`/`core`;
  `std::sync::RwLock` → `crate::sync::RwLock` (il sync layer interno che con
  `custom-sync-primitives` va sugli hook `wasmtime_sync_rwlock_*` già
  implementati in `kernel/src/wasm/wt/platform.rs`); il parking di
  `memory.atomic.wait32/wait64/notify` (`parking_spot`, std-only) è deviato su
  tre hook `extern "C"` del kernel: `wasmtime_futex_wait32/wait64/notify`.
  Contratto wait (semantica wasm threads): 0=woken, 1=not-equal, 2=timed-out,
  `timeout_ns < 0` = infinito; il timeout passa relativo in ns (niente
  `std::time::Instant`). La validazione upstream (`validate_atomic_addr`,
  bounds + alignment) è intatta.
- `src/runtime/vm.rs`: `mod parking_spot` gated a
  `all(feature = "threads", feature = "std")`.

Lato kernel:

- `kernel/Cargo.toml`: feature `threads` sul dep wasmtime +
  `[patch.crates-io]` → `../third_party/wasmtime45`.
- `kernel/src/wasm/wt/platform.rs`: stub temporanei dei tre hook futex
  (wait → 1 = not-equal senza mai bloccare, notify → 0) per far linkare il
  kernel — saranno sostituiti da `wt/threads.rs` nel Task 4.
- `kernel/src/wasm/wt/mod.rs` (`engine_config`): `config.wasm_threads(true)`.

Lato host tool:

- `tools/wt-precompile`: feature `threads` (wasmtime STOCK crates.io — è un
  tool std in workspace separato) + `config.wasm_threads(true)` accanto a
  `epoch_interruption` (la Config deve restare identica campo per campo a
  quella del kernel).

## Perché

MT Fase 2 (spec `docs/superpowers/specs/2026-06-12-wasm-mt-fase2-threads-design.md`):
eseguire app wasm32-wasip1-threads (`std::thread`/rayon) come fiber cooperative
M:N. Upstream `threads` implica `std` (parking_spot usa `std::thread::park` e
`std::time::Instant`): in un kernel no_std il blocking deve passare dal
kernel stesso, da cui il fork con gli hook futex. Task 0 = solo toolchain +
fork + build verde; lo scheduler fiber e i veri hook arrivano nei task
successivi.

Nota compatibilità `.cwasm`: il check a deserialize è un check di SOTTOINSIEME
(`Metadata::check_features`: le feature del modulo devono essere ⊆ di quelle
dell'engine), quindi i `.cwasm` esistenti (compilati senza THREADS) restano
caricabili sull'engine con THREADS attivo — nessun re-AOT necessario (a
differenza del precedente changelog 455, dove cambiava una tunable hashata).
Viceversa i `.cwasm` prodotti dal nuovo wt-precompile registrano il bit THREADS
e NON caricherebbero su un kernel vecchio. Verificato con `make run-test` →
`TEST_PASS`. Target rustup aggiunto in WSL: `wasm32-wasip1-threads`.

## File toccati

- third_party/wasmtime45/ (nuovo: vendor wasmtime 45.0.0 + patch in
  Cargo.toml, src/runtime/vm.rs, src/runtime/vm/memory/shared_memory.rs)
- kernel/Cargo.toml
- kernel/Cargo.lock
- kernel/src/wasm/wt/mod.rs
- kernel/src/wasm/wt/platform.rs
- tools/wt-precompile/Cargo.toml
- tools/wt-precompile/src/main.rs
- CHANGELOG/486-26-06-12-wasmtime-fork-threads-nostd.md
