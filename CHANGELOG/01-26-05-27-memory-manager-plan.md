# 01 — Piano di implementazione gestore memoria

**Data:** 2026-05-27

## Cosa

Scritto il piano di implementazione del sotto-progetto #1 (gestore memoria) in
`docs/superpowers/plans/2026-05-27-memory-manager.md`. Cinque task TDD:

1. Harness self-test seriale (COM1) + supporto build `memory/*.c` + hook al boot.
2. Frame allocator fisico (bitmap da E820).
3. API paging (pagine 4 KiB) + spazi di indirizzamento.
4. Heap kernel buddy allocator.
5. Integrazione: syscall memoria → heap, malloc/free, comando shell `memtest`.

## Perché

Tradurre la spec di design in passi eseguibili e verificabili, ognuno con build +
run su QEMU e output seriale PASS/FAIL.

## File toccati

- docs/superpowers/plans/2026-05-27-memory-manager.md
- CHANGELOG/01-26-05-27-memory-manager-plan.md
