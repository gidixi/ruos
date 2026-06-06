# 305 — Controlli finestra macOS (pallini) + taskbar + protocollo configure

**Data:** 2026-06-06

## Cosa

Controlli finestra "tipo Wayland" lato compositor kernel, con UI stile macOS.

**Pallini in titlebar** (ruos-window `titlebar`, a destra; titolo a sinistra):
🔴 chiudi · 🟡 minimizza · 🟢 maximize/zoom. egui disegna e hit-testa i pallini,
poi `frame_once` inoltra gli intent alle host fn (come già fa il move CSD).

**Protocollo configure (kernel→app)** — autorità sulla dimensione invertita:
- prima: l'app committava la sua superficie a dimensione fissa, il kernel adottava
  `rect.w/h` da lì;
- ora: il kernel possiede `rect.w/h`; l'app legge `wm.window_size()` e renderizza a
  quella. Bootstrap dal primo commit (`Window.sized`), poi il kernel comanda
  (maximize/restore). È ciò che fa funzionare il maximize davvero (re-render, non
  stretch). Le 6 app NON cambiano (passano la loro W/H come default a `frame_once`).

**Stati finestra** (kernel `Window`): `minimized` (nascosta dal composite + dal
hit-test, resta in `wins`), `maximized` + `saved_rect` (work-area sotto il pannello,
`WORKAREA_TOP=32`), `sized`. Helper `toggle_maximize`/`activate`/`focus_topmost_visible`.

**Taskbar** (shell): nuove host fn `wm.window_list` (snapshot globale id/flags/title,
pubblicato dal loop perché le host fn non vedono `wins`) + `wm.activate(id)`.
`gui_core::shell_chrome` disegna una voce per finestra (● aperta / ○ minimizzata,
focus evidenziato, nomi dal CATALOG); click → `wm.activate` (ripristina+raise+focus).

**Nuove host fn `wm`**: `window_size`, `minimize`, `toggle_maximize`, `activate`,
`window_list`.

## Perché

Rendere le finestre controllabili dal compositor (minimize/maximize/restore +
switcher), come in un desktop reale. Kernel-side perché in Model A ogni finestra è
un `egui::Context` separato su una superficie sola: il window management di egui
(intra-Context) non vede schermo/altre finestre → la geometria cross-finestra la
possiede il compositor. egui resta per disegnare i widget di controllo.

Doc: `ruos-desktop/docs/adding-an-app.md` (guida passo-passo per una nuova app).

## File toccati

- kernel/src/wasm/wt/wm.rs (stati `Window`, host fn, configure, taskbar snapshot, loop)
- ruos-desktop/crates/ruos-window/src/lib.rs (titlebar pallini, `frame_once` configure, wrapper)
- ruos-desktop/crates/gui-core/src/desktop/shell.rs (taskbar in `shell_chrome` + `ShellTaskEntry`)
- ruos-desktop/crates/gui-core/src/desktop/mod.rs (re-export)
- ruos-desktop/apps/shell/src/lib.rs (window_list → taskbar → activate)
- ruos-desktop/docs/adding-an-app.md (nuova guida), ruos-desktop/README.md (pointer)
