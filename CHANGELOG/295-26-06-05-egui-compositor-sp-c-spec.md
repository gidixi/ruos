# 295 — Spec SP-C: window-SDK + meccanismo kernel (`wm.spawn` + finestra-di-sfondo)

**Data:** 2026-06-05

## Cosa
Spec del sotto-progetto fondante del **Modello A** ("compositor = il desktop, cresce
come progetto userspace"; il kernel WM diventa solo meccanismo). Brainstorm
approvato: kernel = meccanismo, desktop+app = progetto userspace.

SP-C = due parti:
- **Meccanismo kernel** (`kernel/src/wasm/wt/wm.rs`): host fn **`wm.spawn(name)`** (una
  finestra chiede al kernel di lanciarne un'altra; carica `/bin/<name>.cwasm` dal VFS,
  cache per nome, ritorna il window-id — deferred dopo `frame_all` per evitare mutazione
  re-entrante di `wins`). **Finestra-di-sfondo**: flag `bg` su `Window` (full-screen,
  z=fondo, senza decorazioni, non chiudibile/spostabile, input solo dove nessuna app
  copre) + host fn **`wm.set_background()`** (la finestra si auto-flagga). **Rimpicciolimento
  WM**: via `draw_launcher`/`launcher_hit` + uso di `APPS` come launcher (catalogo →
  shell SP-D); restano lista finestre/compositing/input/CSD/`spawn_app`.
- **Window-SDK** (lib `ruos-window` in `ruos-desktop`): estrae da `compositor-app` le parti
  riusabili (`Platform`-su-`wm` + `titlebar()` CSD + driver `frame()` + binding `wm.*`).
  Un'app = piccolo bin wasip1 = sua UI egui + `run(title, ui)`. UI portabile → dev su PC
  via `pc-backend`. `compositor-app` rifattorizzato per usare l'SDK (+ bottone "spawn
  another" per testare `wm.spawn`).

Verifica: `wm.spawn` da una finestra → nuova finestra; finestra `bg` full-screen dietro;
demo-su-SDK rende ancora; UI su PC. Boot-check + screendump + VBox.

## Perché
Fondazione del Modello A: l'SDK riusabile + il meccanismo (`wm.spawn`, sfondo) su cui
SP-D (shell userspace) ed SP-E (porta le app + ritira gui.cwasm) si appoggiano.

## File toccati
- docs/superpowers/specs/2026-06-05-egui-compositor-sp-c-window-sdk-design.md
