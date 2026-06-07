# 297 — Spec SP-D: desktop shell userspace

**Data:** 2026-06-05

## Cosa
Spec di SP-D (Modello A): il **desktop shell userspace** — `shell.cwasm` = panel +
wallpaper + launcher come finestra di sfondo full-screen; click sul launcher →
`wm.spawn(app)` (finestra compositor separata); il kernel boota nello shell.

Tre parti:
- **gui-core (refactor piccolo, resta puro):** `pub fn shell_chrome(ctx, apps,
  clock) -> ShellIntents { launches, poweroff }` — disegna panel (launcher+clock) +
  wallpaper, ritorna intenti (no dipendenza `wm`); riusa `wallpaper::paint` + un
  panel-draw fattorizzato. Il path `Desktop`/`panel::show` open-toggle resta INTATTO
  (gui.cwasm fullscreen funziona ancora finché SP-E non lo ritira).
- **Crate `shell` (su ruos-window):** reactor wasip1; `frame()` chiama
  `shell_chrome`, mappa `launches`→`ruos_window::spawn(id)`, `poweroff`→`wm.poweroff`;
  primo frame `set_background()` + `wm.surface_size()` (full-screen); usa
  `frame_once_bare` (senza titlebar CSD — è lo sfondo). Catalogo app qui.
- **Kernel:** `Compositor::new` spawna `shell` come finestra iniziale (bg) invece di
  egui-demo; nuovi host fn `wm.poweroff()` + `wm.surface_size()->(w,h)`; ship
  `shell.cwasm` in `/bin`.

Catalogo: elenca le app reali (egui-demo/about/files/terminal/system); egui-demo
spawna ora (prova), le altre man mano che SP-E ne shippa i `.cwasm` (`wm.spawn`→0
no-op finché mancano).

Verifica: boot → desktop (wallpaper+panel/launcher+clock) di sfondo; click egui-demo
→ finestra sopra; gui.cwasm fullscreen ancora ok; boot-check + screendump + VBox.

## Perché
Porta la UX desktop (look di gui.cwasm) in userspace come shell del compositor;
base per SP-E (porta le app reali come finestre + ritira gui.cwasm).

## File toccati
- docs/superpowers/specs/2026-06-05-egui-compositor-sp-d-desktop-shell-design.md
