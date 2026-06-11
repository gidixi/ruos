# 461 — Icone reboot/console: SVG provato, scelto vettoriale coerente

**Data:** 2026-06-11

## Cosa

Provato a usare gli SVG iniettati (svgrepo restart + console) per i pulsanti
riavvio/torna-alla-console del pannello shell, come l'icona menu. A dimensione
pannello (~16px) rendevano pesanti/pieni (la console = box bianco pieno, il
restart = card squadrata) → stonavano col simbolo power a stroke sottile.

Scelta finale: **icone vettoriali a stroke sottile**, coerenti tra loro e col
power (anello). 

- `gui-core/desktop/icons.rs`:
  - `paint_reboot`: punta a **chevron aperto** (due segmenti) invece di triangolo
    pieno → più leggera, coerente.
  - `paint_terminal`: cornice schermo + **un solo prompt `>`** centrato (tolto il
    cursore `_` che intasava a 16px).
- `gui-core/desktop/shell.rs`: i pulsanti reboot/console usano sempre il disegno
  vettoriale (`icons::reboot_button`/`console_button`), niente texture.
- `apps/shell`: rimosso l'embedding/stash dei PNG reboot/console (resta solo
  l'icona menu).
- SVG ritoccati comunque (restart→bianco 48px, console→48px) + PNG rigenerati con
  `xtask`, e restano in `assets/icons/` come riferimento (non usati a runtime).
  `images::icon_button` + chiavi `REBOOT_ICON`/`CONSOLE_ICON` restano disponibili
  per un futuro set di icone-immagine adatte (stroke sottile).

## Perché

Richiesta utente: usare gli SVG come per il menu. Resa visiva scadente a
dimensione pannello → ripiegato su un set vettoriale coerente.

## File toccati
- ruos-desktop/crates/gui-core/src/desktop/icons.rs
- ruos-desktop/crates/gui-core/src/desktop/shell.rs
- ruos-desktop/apps/shell/src/lib.rs
- ruos-desktop/crates/gui-core/src/images.rs (icon_button + chiavi, non usati)
- ruos-desktop/assets/icons/{restart,console}-svgrepo-com.{svg,png}
