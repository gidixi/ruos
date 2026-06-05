# 293 — Spec SP-B: egui-reactor harness + Client-Side Decorations

**Data:** 2026-06-05

## Cosa
Spec del 2° sotto-progetto verso "app egui reali come finestre del compositor".
Decisione utente: **Client-Side Decorations (CSD)** — l'app egui disegna TUTTA la
finestra (titlebar + testo + [X] + contenuto); il modulo `decor` kernel-side (SSD)
viene rimosso.

SP-B = due parti accoppiate:
- **Compositor (kernel)**: drop `decor` (disegno + `decor::hit`); `compose_window` =
  surface raw; `Window.rect` = finestra intera; nuovo host fn `wm.move(dx,dy)`; input →
  raise+focus + evento alla finestra focused (l'app decide [X]/drag); `proc_exit`/trap
  in `frame()` → `close_requested` → reap (safety-net crash); reactor demo ritirati dal
  launcher (`show_in_launcher` su `AppEntry`, restano per i boot-check); taskbar =
  lista-finestre con chiusura kernel (`wm.close`).
- **egui (`ruos-desktop`)**: nuovo crate (`compositor-app`) su `gui-core` (invariato),
  reactor wasip1: `Platform` su `wm` (`present→wm.commit`, `poll_events→wm.poll_event`,
  `surface_info→480×320`, `wall_clock_secs→wm.wall_seconds`), esporta `frame()`; widget
  **titlebar CSD** riusabile (testo + [X]→`wm.close`, drag→`wm.move`); app demo
  (label + bottone counter).

Verifica: boot-check (`egui demo spawn ok pixels=614400`) + screendump (titlebar egui +
testo + [X]; counter incrementa; drag sposta; [X] chiude; click focus) + VBox.

## Perché
Prima vera finestra egui nel compositor, con le decorazioni disegnate da egui (CSD,
look egui-nativo). Base per SP-C (app system-info).

## File toccati
- docs/superpowers/specs/2026-06-05-egui-compositor-sp-b-csd-harness-design.md
