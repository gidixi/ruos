# 440 — init: persistenza enable/disable su file + reload

**Data:** 2026-06-11

## Cosa
- `service/unitfile.rs`: `to_yaml`/`to_json` (serializzazione Unit per la
  riscrittura del file sorgente) + boot-check roundtrip
  serialize→parse→build.
- `service/mod.rs`: `set_enabled` (registry sync + `UnitReq::Persist` in
  queue), `persist` (riscrive il file via VFS WRITE|CREATE|TRUNCATE, dal
  dispatcher async), `reload` (drop file-sourced non-Running + ricarica;
  Running tengono la config vecchia fino al prossimo restart, warn).
- `executor/mod.rs`: dispatcher gestisce `Persist`/`Reload`.

## Perché
Fase 7 spec init-units: enable/disable persistente al reboot, reload a
caldo della dir config.

## File toccati
- kernel/src/service/unitfile.rs
- kernel/src/service/mod.rs
- kernel/src/service/checks.rs
- kernel/src/executor/mod.rs
