# 438 — init: activate_target topo-ordinato + init_units_task boot/post-boot

**Data:** 2026-06-11

## Cosa
- `service/mod.rs`: `activate_target(t)` — set = enabled del target +
  chiusura transitiva dei requires, topo-sort (ciclo → `Failed(cycle)`),
  avvio in ordine con attesa dep "su" (daemon→Running, oneshot→Exited(0),
  cap 10s), requires fallito → `Failed(dep)` e skip. `is_up` pubblica.
- `executor/mod.rs`: `init_units_task` (BSP) — `load_from_disk` (stub) →
  `activate_target(Boot)` → ~3s → `activate_target(PostBoot)`.
  Verificato in QEMU: boot non bloccato, `unit activation complete`,
  run-test PASS.

## Perché
Fase 6 spec init-units: attivazione a fasi con dipendenze.

## File toccati
- kernel/src/service/mod.rs
- kernel/src/executor/mod.rs
