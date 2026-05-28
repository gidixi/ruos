# 15 — Piano implementazione: heap kernel Rust + global allocator

**Data:** 2026-05-28

## Cosa

Scritto il piano dello Step 4 della roadmap Rust in
`docs/superpowers/plans/2026-05-28-rust-heap-allocator.md`. Due task:

1. Aggiungi deps `talc` + `spin`, dichiara `ALLOCATOR` (`Talck<spin::Mutex<()>,
   ErrOnOom>`) in `kernel/src/memory.rs`, abilita `alloc` in `main.rs`; allocator
   non ancora inizializzato (build green, boot ancora hello-only).
2. Aggiungi richieste Limine `MemoryMapRequest`/`HhdmRequest`, implementa
   `init_heap()` (sceglie primo entry USABLE >= 4 MiB, calcola
   `virt = phys + hhdm`, fa `claim` su talc), wiring in `kmain` con smoke test
   `Box::new(0xCAFEBABE)` + `Vec::from(0..5)`, aggiorna `HELLO` del Makefile
   alla riga alloc; boot test TEST_PASS.

## Perché

Tradurre lo spec Step 4 in passi eseguibili, ognuno verificabile via build/boot.

## File toccati

- docs/superpowers/plans/2026-05-28-rust-heap-allocator.md
- CHANGELOG/15-26-05-28-rust-heap-plan.md
