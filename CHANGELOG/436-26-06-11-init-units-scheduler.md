# 436 — init: scheduler timer polling 1s (monotono + calendario RTC)

**Data:** 2026-06-11

## Cosa
`executor/mod.rs`: `unit_scheduler_task` (BSP) — ogni ~1s confronta
`timer::ticks()` (EveryTicks/BootPlus) o `rtc::to_unix_epoch(now)`
(hourly/daily/weekly) con `next_fire`; se scaduto: `service::start(unit)`,
`last_fire` aggiornato, `next_fire = compute_next` (sempre futuro),
BootPlus si auto-disabilita. `service/mod.rs`: `timers_due_snapshot` +
`timer_fired` (sezioni critiche minime, RTC letto solo dal BSP).

## Perché
Fase 5 spec init-units: attivazione temporizzata cron-like, catch-up
senza doppio scatto, no backfill.

## File toccati
- kernel/src/executor/mod.rs
- kernel/src/service/mod.rs
