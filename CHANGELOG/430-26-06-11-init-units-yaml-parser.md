# 430 — init: UnitDoc + parser YAML-subset + boot-check

**Data:** 2026-06-11

## Cosa
`service/unitfile.rs`: modello intermedio `UnitDoc`/`Val{Str,Bool,List}`
comune ai parser. `service/yaml.rs`: parser line-based (`key: value`,
liste `[a, b]`, commenti `#`), zero dipendenze. `service/checks.rs`:
primo gruppo di self-test `svc-check` (gated `boot-checks`), hook in
`boot/phases/userland.rs` dopo `service::init()`.

## Perché
Fase 2 della spec init-units: config su file richiede parsing no_std
hand-written; i check girano in `make run-test` con
`CARGO_FEATURES=boot-checks`.

## File toccati
- kernel/src/service/unitfile.rs
- kernel/src/service/yaml.rs
- kernel/src/service/checks.rs
- kernel/src/service/mod.rs
- kernel/src/boot/phases/userland.rs
