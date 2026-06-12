# 471 — Kernel event bus + notifiche compositor (v1)

**Data:** 2026-06-11

## Cosa
Bus pub/sub kernel→compositor (`kernel/src/kevent.rs`: ring 64 slot, publish
IRQ-safe zero-alloc, side-table nomi `heapless::String<32>`, lettura a cursore
con rilevamento gap → SUBSCRIBER_OVERFLOW sintetizzato dal lettore); shutdown/
reboot differito annullabile in `power.rs` (`request_poweroff/reboot/cancel/
pending` + task di enforcement embassy `pool_size 2` con guard deadline-match);
publish dai punti kernel: APP_CRASHED (frame-error + spawn-error del
compositor, cause trap/watchdog/spawn-failed), APP_FUEL_EXHAUSTED (out-of-fuel
wasmi, nome dal proc-registry), MEM_LOW (frame allocator, soglia <10% liberi
con isteresi ri-arma >15%); compositor: `drain_kevents` nel run loop + toast
(INFO grigio / WARN ambra, max 3 visibili, FIFO, ~5 s, click = dismiss) +
modale CRIT centrato con countdown da `power::pending()` e Annulla/Esc, input
routato SOLO al modale quando attivo; nuovo `decor::draw_text_at` (testo senza
centratura verticale, per overlay full-screen); ABI: `wm.poweroff`/`wm.reboot`
ora differite+annullabili (docs `wm.md`/`ruos-window.md` + commenti SDK
aggiornati nello stesso change; `ruos:gui/power` e `ruos.poweroff` console
restano immediati); builtin debug `kev-test [toast|poweroff|reboot|cancel]` +
host fn `ruos.kev_test` (documentata in `ruos.md`); self-test in-boot
`KEVENT_TEST` (boot-checks: RING_LEN+6 publish, verifica ordine seq e lost==6).

## Perché
Notifiche kernel→utente affidabili anche se il desktop egui è morto (rendering
kernel-side `decor`); l'enforcement dello spegnimento non dipende dalla UI
(funziona headless); il ring broadcast con cursori per-lettore prepara gratis
la futura API app-facing `sys.events_poll` (v2, fuori scope).
Spec: docs/superpowers/specs/2026-06-11-kernel-event-bus-design.md.

## File toccati
- kernel/src/kevent.rs (nuovo)
- kernel/Cargo.toml, kernel/Cargo.lock, kernel/src/main.rs
- kernel/src/power.rs
- kernel/src/wasm/wt/wm.rs
- kernel/src/wasm/fiber.rs
- kernel/src/memory/frames.rs
- kernel/src/wasm/host/proc.rs
- kernel/src/boot/phases/devices.rs
- user/shell/src/main.rs
- docs/api/wm.md, docs/api/ruos.md, docs/api/ruos-window.md
- ruos-desktop/crates/ruos-window/src/lib.rs (submodule)
- docs/superpowers/specs/2026-06-11-kernel-event-bus-design.md (stato + kind TEST)
- docs/superpowers/plans/2026-06-11-kernel-event-bus.md (nuovo, piano)
