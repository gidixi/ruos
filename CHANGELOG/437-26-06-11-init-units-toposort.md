# 437 — init: topo-sort Kahn con rilevamento cicli + boot-check

**Data:** 2026-06-11

## Cosa
`service/topo.rs`: `topo_sort(nodes) -> (ordine, ciclici)` — Kahn
deterministico (indice più basso a parità), dipendenze fuori dal set
ignorate (builtin già attivi), residuo non emesso = nodi in ciclo.
Boot-check: ordine semplice, catena transitiva, ciclo a↔b con nodo
indipendente che prosegue, dep esterna ignorata.

## Perché
Fase 6 spec init-units: ordine di attivazione per `activate_target`.

## File toccati
- kernel/src/service/topo.rs
- kernel/src/service/checks.rs
- kernel/src/service/mod.rs
