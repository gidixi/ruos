# 303 — Compositor perf: commit-on-damage + present-skip + hlt (no recompose-all)

**Data:** 2026-06-06

## Cosa

Il compositor kernel-side non ricompone più tutto ad ogni frame. Tre leve:

- **Leva 0 — niente clone per-frame.** `Compositor::compose_window` ora ritorna un
  puntatore **in prestito** alla surface committata (`*const u8 + len`) invece di
  `pixels.clone()`. `present` compone le bande SMP direttamente da quei puntatori:
  i pixel vivono nello Store di ogni finestra e non sono mutati tra la raccolta e
  il join di `dispatch_bands`, quindi il prestito è valido per tutto il composite.
  Elimina N copie di superficie intera per frame.

- **Leva 1 — present solo su danno.**
  - Lato app (`ruos-window::frame_once`/`frame_once_bare`): `wm.commit` **solo se**
    il dirty-rect del renderer non è vuoto (porta la logica già in `Gui::frame`).
    Una finestra idle non committa nulla e mantiene la sua ultima surface.
  - Lato kernel: `WmState.committed` (settato da `wm.commit`) + `Compositor.dirty`
    (settato dai cambi di geometria: `raise`/`drag_to`/`remove_at`/bg-pin). Il loop
    chiama `present()` **solo se** qualche finestra ha committato o la geometria è
    cambiata. Desktop fermo ⇒ zero composite/blit.
  - **Warm-up:** i primi 90 frame forzano comunque il present, così il band-pool
    SMP scalda gli AP (servono vari frame compositati) e il marker boot
    "composite cores ≥2" resta valido anche con app statiche.
  - **Idle pacing:** lo spin da 2.000.000 iterazioni è sostituito da `hlt` —
    il core dorme fino al prossimo IRQ (timer 100 Hz + input PS/2/USB), invece di
    restare al 100% anche a schermo fermo. Drag/click restano reattivi (IRQ input).

Il cursore è gestito indipendentemente dal layer `gfx` (`fold_mouse`/`blit`), quindi
saltare il present su frame idle non lo congela.

## Perché

Ogni iterazione del loop ricomponeva e ri-blittava l'intero schermo, clonava ogni
surface, e bruciava un core in busy-spin — anche senza nulla che cambiasse. Su
desktop fermo questo è lavoro inutile. Ora idle ⇒ ~zero lavoro grafico + core che
dorme; il composite parte solo quando qualcosa cambia davvero.

ABI invariata: nessun cambio alla firma `frame()`, nessun tocco ai 6 crate app —
il kernel deduce il danno dalla chiamata a `wm.commit`, e clock/animazioni
continuano a funzionare perché `frame_all` chiama comunque `frame()` su tutte le
finestre (il diff del renderer cattura i cambi).

Verificato: `make iso` verde; boot in VirtualBox OK (desktop, spawn app, drag/raise/
close, clock, reattività).

## Follow-up (non in questo commit)

- **Leva 1-bis:** non chiamare nemmeno `frame()` sulle finestre idle, via host fn
  `wm.set_repaint_after` da `out.repaint_after` (modello eframe).
- **Leva 2:** damage regionale — far passare il dirty-rect attraverso `wm.commit`
  e comporre/blittare solo il sotto-rettangolo cambiato.

## File toccati

- kernel/src/wasm/wt/wm.rs
- ruos-desktop/crates/ruos-window/src/lib.rs (submodule)
