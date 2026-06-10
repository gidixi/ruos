# 410 — Dipendenza gzip-core no_std nel kernel

**Data:** 2026-06-10

## Cosa
Aggiunta dipendenza `gzip-core` (path `../user/gzip-core`, `default-features = false`)
nel `[dependencies]` di `kernel/Cargo.toml`, posizionata in ordine alfabetico subito
dopo `getrandom`.

## Perché
Task 5 del piano bin.bgz: il kernel deve poter decomprimere l'archivio `bin.bgz`
(deflate/gzip) al boot. Aggiungere il crate ora permette al Task 6 di usare
`gzip_core::decompress` senza ulteriori modifiche al manifest.
Il flag `default-features = false` esclude il feature `std` (default del crate)
garantendo la build `no_std` necessaria per il kernel bare-metal.

## File toccati
- kernel/Cargo.toml
- kernel/Cargo.lock
