# 428 — Piano di implementazione init system (units, timers, supervisione)

**Data:** 2026-06-11

## Cosa
Piano a 15 task per implementare la spec
`2026-06-09-init-units-timers-design.md` (unica spec approvata non ancora
implementata nel repo): modello Unit/Timer, parser YAML-subset/JSON,
schedule_parse + compute_next su epoch, runner pool daemon con
restart/backoff, scheduler 1s, topo-sort + activate_target, load_from_disk,
persistenza enable/disable, host ABI `ruos.unit_*`, tool `unitctl`,
boot-checks TDD.

## Perché
Ciclo spec → piano → implementazione (regola repo). Verificato prima che le
altre spec candidate (demand paging, app-sleep) risultano già implementate
nel codice.

## File toccati
- docs/superpowers/plans/2026-06-11-init-units-timers.md
