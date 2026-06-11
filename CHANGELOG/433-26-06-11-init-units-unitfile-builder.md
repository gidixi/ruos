# 433 — init: builder UnitDoc→Unit|Timer + boot-checks verdi in QEMU

**Data:** 2026-06-11

## Cosa
`service/unitfile.rs`: `build(doc, file) -> Parsed::{U(Unit),T(Timer)}` —
`kind: timer` discrimina, defaults spec (type=oneshot, restart=no,
target=manual, enabled=false), validazione valori, chiavi sconosciute
warn-only. Boot-check `svc-check: unitfile OK`. Verifica fase parser in
QEMU: `make run-test` con `CARGO_FEATURES=boot-checks` PASS (yaml, json,
schedule, unitfile).

## Perché
Fase 2 spec init-units completata: file config → modello runtime.

## File toccati
- kernel/src/service/unitfile.rs
- kernel/src/service/checks.rs
