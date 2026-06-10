# 406 — gzip-core: no_std con feature opt-in std

**Data:** 2026-06-10

## Cosa
- `user/gzip-core/Cargo.toml`: dipendenza `miniz_oxide` aggiornata a `default-features = false, features = ["with-alloc"]`; aggiunti `[features]` con `default = ["std"]` e `std = []`.
- `user/gzip-core/src/lib.rs`: sostituito con versione `no_std`-capable (`#![cfg_attr(not(feature = "std"), no_std)]`, `extern crate alloc`, `mod cli` / `pub use cli::run_cli` gated su `#[cfg(feature = "std")]`, `pub mod pack` aggiunto).
- `user/gzip-core/src/format.rs`: aggiunti `use alloc::boxed::Box` e `use alloc::vec::Vec` dopo `use core::fmt`.
- `user/gzip-core/src/pack.rs`: creato placeholder (Task 2 lo sovrascriverà).

## Perché
Il kernel (`no_std`) deve poter dipendere da `gzip-core` per decomprimere archivi
`.bgz`; i bin userland (`gzip`/`gunzip`/`zcat`) continuano ad usare la feature `std`
che abilita la CLI. Build verificata su `wasm32-unknown-unknown --no-default-features`
e test host 18/18 verdi.

## File toccati
- user/gzip-core/Cargo.toml
- user/gzip-core/src/lib.rs
- user/gzip-core/src/format.rs
- user/gzip-core/src/pack.rs
