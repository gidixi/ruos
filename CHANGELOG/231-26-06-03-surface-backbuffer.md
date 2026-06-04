# 231 — Surface: RAM back-buffer + dirty blit

**Data:** 2026-06-03

## Cosa
Aggiunto `kernel/src/console/surface.rs`: struct `Surface` che mantiene un
back-buffer in RAM con layout identico al framebuffer (pitch, bpp, pixel order)
e un metodo `blit_rect` che copia span dirty su MMIO via `write_volatile`.
`blit_rect` è no-op quando `addr` è null, così è testabile in RAM senza
framebuffer reale.

Aggiunte asserzioni T18/T19 in `engine_test.rs`: put_px + read_px su BGR 32bpp
con addr=null.

## Perché
Task 6 del piano terminal-engine: la `Surface` è l'unica unità autorizzata a
scrivere sul framebuffer; disaccoppia il rendering (Task 7) dall'I/O MMIO e
permette test in RAM.

## File toccati
- kernel/src/console/surface.rs (nuovo)
- kernel/src/console/mod.rs (aggiunto `pub mod surface;`)
- kernel/src/console/engine_test.rs (T18, T19)
- CHANGELOG/231-26-06-03-surface-backbuffer.md
