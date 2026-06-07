# 303 — SMP Step 1: allocator architecture decision record (data-driven, pending confirmation)

**Data:** 2026-06-05

## Cosa
Decision record per l'architettura allocatore SMP Step 1, basata su misure reali a
`-smp 4` (QEMU). Tre varianti benchmarkate: default talc globale, Prototype A
(magazine per-core), Prototype B (per-core talc + remote-free). Risultato chiave:
entrambi i prototipi sono **più lenti** del talc globale sotto contesa a 4 core,
a causa del costo `cpu_id()` via LAPIC MMIO (~200 ns/call non mitigato). La decisione
finale è PENDING conferma del controller.

## Perché
Chiude lo spike Step 1 con un documento di confronto dati che alimenta la scelta
produzione dell'allocatore. Le misure confirmano il rischio §10 già identificato nella
spec (§6): senza `cpu_id()` veloce (gs-base), le arene per-core sono più lente del
lock globale. Il decision record presenta 4 opzioni (adottare A, adottare B, costruire
fast-cpu_id prima, mantenere default) con evidenza; non dichiara unilateralmente il
vincitore.

## File toccati
- `docs/superpowers/decisions/2026-06-05-allocator-architecture.md` (nuovo)
- `CHANGELOG/303-26-06-05-smp-step1-allocator-decision.md` (questo file)
