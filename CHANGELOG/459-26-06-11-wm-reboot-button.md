# 459 — Pulsante riavvio nel compositor

**Data:** 2026-06-11

## Cosa

Aggiunto un pulsante **riavvia** nella barra del desktop (shell SP-D), gemello
del pulsante di spegnimento, con icona vettoriale (freccia circolare) disegnata
a mano come quella power.

- `gui-core/desktop/icons.rs`: `reboot_glyph` + `paint_reboot` + `reboot_button`
  (arco con gap in alto + punta a freccia), con test.
- `gui-core/desktop/shell.rs` (`shell_chrome`, path on-device Model A):
  `ShellIntents.reboot` + pulsante accanto a power.
- `gui-core/desktop/panel.rs` + `desktop/mod.rs` + `app.rs` + `lib.rs` +
  `platform.rs` + `pc-backend`: stesso pulsante/intent nel path monolitico
  (anteprima PC / `gui.cwasm`), `Platform::reboot` (default no-op; PC → exit).
- `ruos-window`: import `wm.reboot` + wrapper `reboot()`.
- `apps/shell`: mappa l'intent `reboot` su `ruos_window::reboot()`.
- Kernel `wasm/wt/wm.rs`: host fn `wm.reboot` → `crate::power::reboot()`
  (gemella di `wm.poweroff`).
- `docs/api/wm.md`: documentata `reboot()`.

## Perché

Richiesta utente: avere nel compositor un pulsante per riavviare la macchina,
con icona, come quello di spegnimento.

## File toccati
- ruos-desktop/crates/gui-core/src/desktop/icons.rs
- ruos-desktop/crates/gui-core/src/desktop/shell.rs
- ruos-desktop/crates/gui-core/src/desktop/panel.rs
- ruos-desktop/crates/gui-core/src/desktop/mod.rs
- ruos-desktop/crates/gui-core/src/app.rs
- ruos-desktop/crates/gui-core/src/lib.rs
- ruos-desktop/crates/gui-core/src/platform.rs
- ruos-desktop/backends/pc-backend/src/main.rs
- ruos-desktop/crates/ruos-window/src/lib.rs
- ruos-desktop/apps/shell/src/lib.rs
- kernel/src/wasm/wt/wm.rs
- docs/api/wm.md
