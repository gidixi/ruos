# 472 — Notifiche egui via app overlay + panic screen

**Data:** 2026-06-11

## Cosa
Toast e modale power ora renderizzati in egui vero dalla nuova app overlay
`notify` (full-screen TRASPARENTE, compositata sopra tutte le finestre):
- **Kernel**: flag finestra `overlay` (speculare a `bg`, z-top, mai
  taskbar/focus/hit-test normale, `wm.set_overlay`); alpha-blend src-over
  premoltiplicato nel band kernel (`WinDesc.blend`, tutto intero); routing
  input con hit-test per-pixel sull'alpha (soglia 32) + modal grab totale con
  power pending + tracking `overlay_btn` (un drag di finestra non viene mai
  rubato); spawn di `notify` all'avvio del compositor; decor v1 = FALLBACK
  automatico (notify assente/morta), `tick_modal` riapre il modale decor se
  l'overlay muore col countdown in corso.
- **API host nuove** (docs/api aggiornate): `sys.events_poll` (record 64 B,
  cursore kevent per-finestra, overflow sintetico, rollback su write fallita),
  `wm.power_pending`, `wm.power_cancel`, `wm.set_overlay`.
- **SDK** (`ruos-desktop`, submodule): `Renderer::set_clear` (gui-core),
  `WindowState::new_overlay()` (canvas trasparente), wrapper `events_poll`/
  `power_pending`/`power_cancel`/`set_overlay`, `KEventRec`/`PowerKind`.
- **App `notify`** (`ruos-desktop/apps/notify-app`, no manifest → mai nel
  launcher): toast egui arrotondati con bordo per severity, fade-out, click
  dismiss, max 3 + coda FIFO, ~5 s; modale countdown da `power_pending()` con
  Annulla/Esc; `stay_awake` solo con contenuto attivo. Makefile: regola
  `build/notify.cwasm` + staging in `/bin`.
- **Panic screen** (FATAL, kernel-only, MAI via bus): il panic handler disegna
  direttamente sul framebuffer lineare (atomics gfx, niente lock/alloc):
  sfondo rosso scuro, messaggio+location, core/tick, tail del klog ring
  (`klog::try_read` nuovo, non-bloccante, buffer statico LOG_CAP), footer;
  default 30 s di schermo (busy-wait TSC) poi reboot; `panic-halt` → halt.
  Guard anti-rientranza nel panic handler (doppio panic → hlt). Debug:
  `kev-test panic` (mode 4 di `ruos.kev_test`).

Hardening da review finale: focus mai sull'overlay (fix nel deferred apply +
`remove_at` ripiega su `focus_topmost_visible` se il focus slitta su
bg/overlay); il modal grab che ruba la release di un drag resetta
`btn_l`/`drag` (niente drag fantasma post-Annulla); `PANIC_FREEZE` in gfx
(blit/cursore no-op dopo il panic screen — il core GUI non ridipinge sopra);
cap 32 sulle code toast (kernel decor + app); `MAX_WINDOWS` 8→9 (notify
occupa uno slot stabile); Id egui dei toast chiave su `seq` (stabile allo
scadere dei precedenti); `events_poll` con rollback del cursore su write
guest fallita.

## Perché
Estetica egui vera per le notifiche senza perdere la garanzia v1 (CHANGELOG
471): il decor kernel resta fallback per compositor/notify morti e
l'enforcement dello shutdown resta nel task kernel; i FATAL (panic) non
possono dipendere da WASM/executor → path sincrono direct-framebuffer.
Spec: docs/superpowers/specs/2026-06-11-notifiche-egui-overlay-design.md.

## File toccati
- kernel/src/wasm/wt/compose.rs (WinDesc.blend + blend branch)
- kernel/src/wasm/wt/wm.rs (overlay window + routing + host fn)
- kernel/src/gfx/mod.rs (panic_screen)
- kernel/src/klog.rs (LOG_CAP pub + try_read)
- kernel/src/main.rs (panic handler: screen + guard + reboot differito 30 s)
- kernel/src/wasm/host/proc.rs (kev_test mode 4)
- user/shell/src/main.rs (kev-test panic)
- Makefile (build/notify.cwasm + iso + binstage)
- docs/api/wm.md, docs/api/sys.md, docs/api/ruos.md, docs/api/ruos-window.md
- ruos-desktop/ (submodule): Cargo.toml, apps/notify-app/* (nuova),
  crates/gui-core/src/raster.rs (set_clear), crates/ruos-window/src/lib.rs
- docs/superpowers/specs/2026-06-11-notifiche-egui-overlay-design.md (stato)
- docs/superpowers/plans/2026-06-11-notifiche-egui-overlay.md (piano, nuovo)
