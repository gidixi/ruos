# 277 — Compositor GATE PASSES (multi-istanza reactor + surface + 2 finestre)

**Data:** 2026-06-05

## Cosa
Provato il **gate** del compositor multi-finestra kernel-side (spec 276): il
kernel tiene **2 istanze wasm reactor persistenti**, chiama il loro `frame()`
esportato a turno, legge la **surface** committata di ognuna e le **compone
affiancate** nel framebuffer. Schermo: **due finestre colorate side-by-side**,
con colori diversi (offset per-id) che ciclano. De-rischia l'intera direzione del
compositor.

## Prove
- `reactor spike frame-calls=5` (boot-check): istanza wasm persistente +
  `frame()` chiamato 5× (`get_typed_func` + `.call()` ripetuto). Era il rischio
  vero — **funziona** in no_std AOT.
- `reactor spike calls=5 commit_b0=0x05 pixels=307200`: la surface committata
  (320×240×4) arriva nel kernel intatta col byte colore atteso.
- Screendump QEMU+KVM: 2 finestre affiancate (origini (0,0) e (w/2,0)), colori
  distinti per-id. Nessun panic. Self-test esistenti invariati.

## Come
- Guest reactor `tools/wt-reactor` (no_std, `wasm32-unknown-unknown`, niente
  WASI): esporta `frame()`, importa `wm.{commit,app_id,tick}`; riempie un buffer
  statico (colore = f(counter, id)) e lo committa ogni frame.
- Kernel `kernel/src/wasm/wt/wm.rs`: `WmState` per-istanza + host module `wm` +
  `run_compositor_gate` (2 `Store`/`Instance`, round-robin `frame()`, blit di ogni
  surface nella sua metà-schermo via `crate::gfx::blit`). Riusa blit fast-path +
  dirty-rect + cursore.
- Launch: `/bin/compositor.cwasm` (= reactor cwasm via `limine.conf` module),
  router exec lo instrada a `run_compositor_gate`; `compositor-init.sh` lo lancia.
- **Modello reactor**: il kernel guida il loop (l'app esporta `frame()`, non fa il
  loop) → cooperativo single-CPU, niente fiber. Raw `wm` import per il gate; WIT
  `surface` quando si costruisce il compositor vero.

## Cosa NON c'è ancora (prossimi sotto-progetti)
Input/focus routing → window manager (drag/resize/z-order/decorazioni) →
compositing parallelo SMP → launcher/lifecycle. Ognuno spec→piano→build.

## Nota
`/bin` è popolato dai boot-module di Limine (`limine.conf`), non dai file
sull'ISO → serve l'entry module per `/bin/compositor.cwasm`.

## File toccati
- tools/wt-reactor/* (guest)
- kernel/src/wasm/wt/wm.rs (nuovo), kernel/src/wasm/wt/mod.rs
- kernel/src/boot/phases/interrupts.rs, kernel/src/executor/mod.rs
- Makefile, limine.conf, user-bin/compositor-init.sh
