# 14 — Spec design: heap kernel Rust + global allocator (talc)

**Data:** 2026-05-28

## Cosa

Scritta la spec del milestone Step 4 in
`docs/superpowers/specs/2026-05-28-rust-heap-allocator-design.md`: heap kernel
4 MiB su RAM reale via Limine memory map + HHDM, `#[global_allocator]` `talc`
con `spin::Mutex` + `ErrOnOom`. Nuovo modulo `kernel/src/memory.rs`
(`init_heap` + `HeapInfo` + `HeapInitError`), nuove richieste Limine
(`MemoryMapRequest` + `HhdmRequest`) bracket dai marker esistenti. Smoke test
in `kmain`: `Box::new(0xCAFEBABE)` + `Vec::from(0..5)` con log seriale; il
target `run-test` asserisce la riga alloc.

## Perché

Step 4 della roadmap Rust: abilita `alloc` (Box/Vec/String/BTreeMap) per ogni
strato successivo (IDT/GDT, frame allocator, scheduler, VFS).

## File toccati

- docs/superpowers/specs/2026-05-28-rust-heap-allocator-design.md
- CHANGELOG/14-26-05-28-rust-heap-spec.md
