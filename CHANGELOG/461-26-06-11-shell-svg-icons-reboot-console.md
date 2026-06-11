# 461 — Icone SVG per i pulsanti riavvio + torna-alla-console

**Data:** 2026-06-11

## Cosa

I pulsanti **riavvio** e **torna alla console** del pannello shell ora usano le
icone SVG iniettate (come l'icona menu), con fallback al disegno vettoriale se la
texture non è disponibile. (Power resta vettoriale: nessun SVG fornito.)

- `assets/icons/restart-svgrepo-com.svg`: ricolorato bianco (#FFFFFF) + ridotto a
  48px per il tinting runtime (era nero 800px → invisibile al tint, e pesante).
- `assets/icons/console-svgrepo-com.svg`: aggiunti `width/height=48px`.
- Rigenerati i PNG committati con `cargo run -p xtask` (SVG→PNG, 48×48):
  `restart-svgrepo-com.png`, `console-svgrepo-com.png`.
- `gui-core/images.rs`: chiavi `REBOOT_ICON` + `CONSOLE_ICON` nello stash + helper
  `icon_button` (immagine monocroma tinta col tema, cliccabile, frameless —
  gemello di `icon_menu` senza popup).
- `gui-core/desktop/shell.rs`: i pulsanti reboot/console usano `images::get` +
  `icon_button` se la texture c'è, altrimenti `icons::reboot_button`/
  `console_button` (vettoriale).
- `apps/shell`: embedda i due PNG (`include_bytes!`), li decodifica con
  `ruos-assets` e li ri-deposita ogni frame via `images::stash`, come l'icona menu.

## Perché

Richiesta utente: usare gli SVG messi negli asset per i pulsanti, come fatto per
l'icona del menu.

## File toccati
- ruos-desktop/assets/icons/restart-svgrepo-com.svg
- ruos-desktop/assets/icons/console-svgrepo-com.svg
- ruos-desktop/assets/icons/restart-svgrepo-com.png (rigenerato)
- ruos-desktop/assets/icons/console-svgrepo-com.png (rigenerato)
- ruos-desktop/crates/gui-core/src/images.rs
- ruos-desktop/crates/gui-core/src/desktop/shell.rs
- ruos-desktop/apps/shell/src/lib.rs
