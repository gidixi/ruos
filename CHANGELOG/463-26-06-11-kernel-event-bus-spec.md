# 463 — Spec: kernel event bus + notifiche compositor (v1)

**Data:** 2026-06-11

## Cosa

Scritta la spec di design del sistema di notifiche kernel→compositor:
ring buffer broadcast `kevent` (publish IRQ-safe zero-alloc, seq monotonico,
overflow detection per-lettore), catalogo eventi v1 (`APP_CRASHED`,
`APP_FUEL_EXHAUSTED`, `SHUTDOWN/REBOOT_PENDING`, `POWER_CANCELLED`,
`MEM_LOW`, `SUBSCRIBER_OVERFLOW`), shutdown/reboot differito annullabile
(countdown 10 s, enforcement via task async indipendente dalla UI, cambio
semantica `wm.poweroff`/`wm.reboot`), rendering kernel-side nel compositor
(toast INFO/WARN via `decor`, modale CRIT con bottone Annulla). API
app-facing e `/dev/kevents` rimandati a v2.

Solo spec: nessun codice implementato.

## Perché

Oggi crash/fuel-out di un'app WASM e lo spegnimento sono invisibili o
istantanei: serve un canale pub/sub kernel→UI per dare feedback visivo
all'utente, con percorso critico (toast/modale kernel-side) che funzioni
anche se il desktop egui è morto.

## File toccati

- docs/superpowers/specs/2026-06-11-kernel-event-bus-design.md
