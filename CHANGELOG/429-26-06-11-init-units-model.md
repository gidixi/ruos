# 429 — init: modello Unit/Timer, registry a String, queue multi-request

**Data:** 2026-06-11

## Cosa
`service/mod.rs`: `Service`→`Unit` (kind oneshot/daemon, restart policy,
after/requires, target boot/post-boot/manual, enabled, restarts,
stop_requested, file sorgente), `UnitStatus` (+`Restarting`), `Timer` +
`Schedule` (enum in `service/schedule.rs`), registry `UNITS`/`TIMERS`,
`UnitReq{Start,Persist,Reload}` sulla queue, `stop()` cooperativo via
`proc::request_kill`, `ServiceError` +`NoSlot`/`Parse`. Dispatcher adattato
a nomi owned. Fase 1 della spec init-units-timers.

## Perché
Base dati per l'init system (spec
`2026-06-09-init-units-timers-design.md`, piano
`2026-06-11-init-units-timers.md`).

## File toccati
- kernel/src/service/mod.rs
- kernel/src/service/schedule.rs
- kernel/src/executor/mod.rs
