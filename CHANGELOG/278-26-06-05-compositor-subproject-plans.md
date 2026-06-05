# 278 — Piani dei 4 sotto-progetti compositor + interface contract

**Data:** 2026-06-05

## Cosa
Scritti i piani d'implementazione concreti per i **4 sotto-progetti rimanenti** del
compositor multi-finestra (spec 276, gate 277):
- `2026-06-05-compositor-sp2-input-focus.md` — input routing + click-to-focus + code eventi per-finestra + `wm.poll_event`.
- `2026-06-05-compositor-sp3-window-manager.md` — decorazioni (title bar + [X]), drag, z-order, close.
- `2026-06-05-compositor-sp4-smp-compositing.md` — compositing parallelo sul compute-pool SMP.
- `2026-06-05-compositor-sp5-launcher-lifecycle.md` — launcher (spawn app come processo) + teardown.

## Coerenza
I 4 piani sono stati scritti **in parallelo** (workflow) e sono drift-ati sulle
interfacce condivise (4 forme diverse di `Window`, nome entry-point, posizione dei
pixel, modello z-order, hook `present_frame`). Un pass di coerenza l'ha
diagnosticato. → Scritto un **interface contract autoritativo**
(`2026-06-05-compositor-subprojects-interface-contract.md`) che fissa il modello
canonico (`struct Window`/`Compositor` introdotti in SP2, pixel in
`store.data().pixels`, z = ordine del Vec, cursore = `gfx::mouse_pos()`, entry
`run_compositor_gate` invariato, `present()` parallelizzato in SP4 sui footprint
**decorati**, linker = unione delle host fn, SP5 usa le API reali di SP3). Ogni
piano ha in testa un puntatore "leggi prima il contract".

## Ordine
SP2 → SP3 → SP4 → SP5 (l'ordine di dipendenza è corretto; serve solo che SP2 fondi
i tipi canonici che gli altri estendono). Ognuno: piano → build subagent-driven.

## File toccati
- docs/superpowers/plans/2026-06-05-compositor-sp{2,3,4,5}-*.md (4 piani)
- docs/superpowers/plans/2026-06-05-compositor-subprojects-interface-contract.md
