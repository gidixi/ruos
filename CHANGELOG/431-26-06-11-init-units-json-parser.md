# 431 — init: parser JSON-subset + boot-check

**Data:** 2026-06-11

## Cosa
`service/json.rs`: parser char-scanner per UN oggetto piatto
`{ "k": v }` con v ∈ stringa | bool | numero (tenuto come stringa) |
array di stringhe → `UnitDoc`. Boot-check `svc-check: json OK`.

## Perché
Fase 2 spec init-units: secondo formato config (`.json`) con lo stesso
modello intermedio del parser YAML.

## File toccati
- kernel/src/service/json.rs
- kernel/src/service/checks.rs
- kernel/src/service/mod.rs
