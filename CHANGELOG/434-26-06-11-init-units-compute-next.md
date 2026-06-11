# 434 — init: compute_next calendario su epoch + boot-check rollover

**Data:** 2026-06-11

## Cosa
`service/schedule.rs`: `compute_next(schedule, epoch_now, now_ticks)` —
hourly/daily/weekly su aritmetica unix-epoch (dow da epoch, 1970-01-01 =
giovedì), EveryTicks/BootPlus su tick monotoni. Sempre scatto FUTURO
("fire if due, recompute to future"). Boot-check con casi domani/stessa
ora/settimana attraversata/rollover anno (epoch verificati esternamente).

## Perché
Fase 3 spec init-units. Deviazione dichiarata: epoch invece di RtcTime
iniettato — riusa `rtc::to_unix_epoch`, rollover gratis, stessa
testabilità.

## File toccati
- kernel/src/service/schedule.rs
- kernel/src/service/checks.rs
