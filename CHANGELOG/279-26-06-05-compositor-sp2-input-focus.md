# 279 — Compositor SP2: input routing + click-to-focus

**Data:** 2026-06-05

## Cosa
Implementato + verificato a runtime il sotto-progetto **SP2** del compositor
multi-finestra (piano `2026-06-05-compositor-sp2-input-focus.md`, contract
`2026-06-05-compositor-subprojects-interface-contract.md`).

Lato kernel (`kernel/src/wasm/wt/wm.rs`, `kernel/src/gfx/mod.rs`):
- Refactor del gate nei **tipi canonici** del contract: `struct Window {id, store,
  inst, rect, title, focused, alive}` + `struct Compositor {wins, module, linker,
  focused, drag}`. Pixel in `store.data().pixels`, z-order = ordine del `Vec`,
  entry invariato `run_compositor_gate` → `Compositor::new(cwasm).run()`.
- **Input routing**: il compositor è l'unico consumer di `gfx::pop()`; ogni frame
  fa hit-test del click (button-down) con `window_at(mouse_pos())` → `raise` +
  `set_focus`; gli eventi (coord tradotte in window-local) vanno SOLO nella coda
  della finestra focused (`WmState.events`).
- `gfx::mouse_pos()` (unica sorgente del cursore per l'hit-test).
- **Host fn `wm.poll_event`**: ritorna un `option<gfx-event>` da 20 byte (disc@0,
  kind@4, p0@8, p1@12, p2@16, LE) drenando un evento dalla coda della finestra
  chiamante. Stesso ABI di `ruos:gui/gfx poll-event`.
- Bordo di focus disegnato attorno alla finestra attiva.
- Marker seriale `WM-FOCUS <idx>` in `set_focus` (verifica deterministica).

Lato guest (`tools/wt-reactor/src/lib.rs`): importa `wm.poll_event`, conta i
click (kind==2, p1!=0) e cambia colore di riempimento ad ogni click → la
reazione all'input è visibile.

## Verifica (runtime, QEMU+KVM + QMP)
`make iso ISO=build/comptest.iso INIT_SCRIPT=user-bin/compositor-init.sh`, boot
headless, iniezione mouse via QMP (`build/comp_verify.py`). 3 segnali indipendenti:
- Seriale: `WM-FOCUS 1` dopo il click sulla finestra 1.
- Screendump: il bordo di focus si sposta sulla finestra cliccata (shotA→B→C).
- Pixel: il riempimento della finestra cliccata cambia, l'altra resta invariata
  (input isolato per-finestra). RESULT: PASS.

## Perché
Prima tappa del compositor multi-finestra: dare a ogni app la propria coda
eventi e il focus-follows-click, base per WM (SP3), compositing SMP (SP4) e
launcher (SP5).

## File toccati
- kernel/src/wasm/wt/wm.rs
- kernel/src/gfx/mod.rs
- tools/wt-reactor/src/lib.rs
- build/comp_verify.py
