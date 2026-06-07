# 297 — Step 7 baseline: invarianti globali (commenti)

**Data:** 2026-06-05

## Cosa
Step 7 della migrazione SMP — baseline documentale. Commenti d'invariante
in-code: ordine lock MAPPER→FRAMES, ALLOCATOR è spinlock già-SMP (contesa, non safety,
audit CHANGELOG/186). Nessun cambiamento funzionale.

## Perché
La migrazione SMP shared-nothing parte fissando la baseline corretta: i lock sono già
SMP-safe, il problema è la contesa; ordine lock esplicito per i passi successivi.

## File toccati
- kernel/src/memory/heap.rs
- kernel/src/memory/mapper.rs
- kernel/src/memory/frames.rs
- CHANGELOG/297-26-06-05-smp-step7-baseline-globali.md
