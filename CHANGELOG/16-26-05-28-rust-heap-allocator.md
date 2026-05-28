# 16 — Global allocator dichiarato (talc) + alloc abilitato

**Data:** 2026-05-28

## Cosa
- Aggiunte deps talc + spin a kernel/Cargo.toml.
  Versioni risolte: talc 4.4.3, spin 0.9.8 (transitive: lock_api 0.4.14,
  scopeguard 1.2.0).
- Nuovo modulo kernel/src/memory.rs: ALLOCATOR statico Talck con ErrOnOom,
  costante HEAP_SIZE (4 MiB). API canonica `Talc::new(ErrOnOom).lock()`
  ha funzionato senza adattamenti.
- kernel/src/main.rs: `extern crate alloc;` + `mod memory;`.
- Allocator dichiarato ma non ancora inizializzato; il kernel continua a fare
  solo "hello serial" + halt.

## Perché
Primo passo dello Step 4 della roadmap: rendere disponibile il global allocator
per `alloc` (Box/Vec/String/BTreeMap); l'inizializzazione effettiva arriva
nel task successivo.

## File toccati
- kernel/Cargo.toml
- kernel/src/memory.rs
- kernel/src/main.rs
- CHANGELOG/16-26-05-28-rust-heap-allocator.md
