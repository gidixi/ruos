# 432 — init: schedule_parse + backoff_ticks + boot-check

**Data:** 2026-06-11

## Cosa
`service/schedule.rs`: `schedule_parse` per le 5 sintassi (`hourly :MM`,
`daily HH:MM`, `weekly Dow HH:MM`, `every Ns`, `boot+Ns` — secondi→tick
@100Hz) con validazione range; `backoff_ticks` esponenziale capato
1s,2s,4s,…,30s. Boot-check `svc-check: schedule OK`.

## Perché
Fase 2/4 spec init-units: parsing schedule per i timer e backoff per la
restart policy del supervisor.

## File toccati
- kernel/src/service/schedule.rs
- kernel/src/service/checks.rs
