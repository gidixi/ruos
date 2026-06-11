# 460 — Pulsante "torna alla shell" nel compositor

**Data:** 2026-06-11

## Cosa

Aggiunto un pulsante **torna alla console** nella barra del desktop (shell SP-D):
chiude il compositor (teardown di tutte le finestre) e ridà il framebuffer alla
shell testuale, come un VT-switch Linux. Per tornare alla GUI si rilancia
`compositor` dalla shell.

- `gui-core/desktop/icons.rs`: icona vettoriale `paint_terminal` (schermo + prompt
  `>` + cursore) + `console_button`.
- `gui-core/desktop/shell.rs`: `ShellIntents.exit_to_shell` + pulsante (con tooltip).
- `ruos-window`: import `wm.exit_to_shell` + wrapper `exit_to_shell()`.
- `apps/shell`: mappa l'intent su `ruos_window::exit_to_shell()`.
- Kernel `wasm/wt/wm.rs`:
  - host fn `wm.exit_to_shell` → setta il flag `EXIT_TO_SHELL`.
  - `Compositor::run` ora ritorna (`()` invece di `-> !`): il run loop legge il
    flag a inizio iterazione, esce, fa `self.wins.clear()` + `gfx::leave()`
    (la console riprende il framebuffer e ridisegna il prompt).
  - `run_compositor_gate` ora ritorna; `gui_worker_loop` azzera
    `COMPOSITOR_MAILBOX.ready` dopo il ritorno per attendere il prossimo hand-off
    di `compositor` (il fallback 1-core dell'exec-worker già aveva l'arm `0`).
- `docs/api/wm.md`: documentata `exit_to_shell()`.

## Perché

Richiesta utente: un pulsante per tornare alla shell dal compositor, come fa
Linux. Semantica scelta dall'utente: teardown (non sospensione); ritorno alla GUI
ri-eseguendo il binario `compositor`.

## File toccati
- ruos-desktop/crates/gui-core/src/desktop/icons.rs
- ruos-desktop/crates/gui-core/src/desktop/shell.rs
- ruos-desktop/crates/ruos-window/src/lib.rs
- ruos-desktop/apps/shell/src/lib.rs
- kernel/src/wasm/wt/wm.rs
- docs/api/wm.md
