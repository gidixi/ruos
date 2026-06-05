# 280 — Compositor SP3: window manager (decorazioni + drag/raise/close)

**Data:** 2026-06-05

## Cosa
Implementato + verificato a runtime il sotto-progetto **SP3** del compositor
multi-finestra (piano `2026-06-05-compositor-sp3-window-manager.md`, contract
`2026-06-05-compositor-subprojects-interface-contract.md`). Estende SP2 (input +
focus, CHANGELOG 279) sopra il gate (CHANGELOG 277). Codice kernel-side in
`kernel/src/wasm/wt/wm.rs` (commit `363d1a2` SP3-A geometria/disegno/state-machine
+ `420bfd0` SP3-B per-frame loop + present() back-buffer).

Lato kernel (`kernel/src/wasm/wt/wm.rs`):
- **Decorazioni finestra** (modulo `decor`, geometria pura unit-checkable):
  **title bar** alta `TITLE_H=28` sopra la surface, con **testo titolo** bianco
  (font noto bitmap, `draw_text` + `blend_glyph` alpha-blend), e un bottone
  **[X]** rosso quadrato (`BTN_W=TITLE_H`) all'estremità destra della barra.
  Colore barra **focus-dipendente**: blu (`BAR_FOCUSED`) per la finestra attiva,
  grigio (`BAR_UNFOCUSED`) per le inattive. `compose_window(idx)` rasterizza il
  footprint completo (barra decorata + surface) in un buffer RGBA8888.
- **Title-bar drag**: state-machine `DragState {win_id, grab_dx, grab_dy}`.
  Mousedown sulla barra → cattura il **grab offset** (cursore − origine
  footprint), `raise`+`set_focus`, inizia drag; mousemove → `drag_to` trasla la
  surface seguendo il cursore senza salti; mouseup → fine drag. `drag_to` clampa
  il footprint perché la barra resti **interamente on-screen**.
- **Z-order raise-on-click**: `raise(idx)` sposta la finestra in coda al `Vec`
  `wins` (move-to-top); ogni click (barra o surface) della finestra topmost sotto
  il cursore (`topmost_decor_at`, hit-test sul footprint) la porta in primo piano
  + la mette a fuoco. Z-order = ordine del `Vec` (nessun campo `z`).
- **[X]-close**: hit sul bottone `[X]` → `close(id)` rimuove il `Window` dal
  `Vec`, il che droppa `(Store, Instance)` → **tear-down dell'istanza wasm**.
- **Per-frame `present()`** con **back-buffer** kernel: ogni frame azzera il
  back-buffer al colore desktop (`DESKTOP_BG`), ricompone tutte le finestre in
  z-order (`compose_window` → `blit_into`), poi **un solo** `gfx::blit`
  full-screen. Il clear-per-frame rende drag e close **ghost-free** (la finestra
  chiusa non lascia residui).
- **Boot-check `wm_logic_selftest`** (feature `boot-checks`): esercita
  geometria + hit-test + z-order + drag-math senza istanziare wasm; marker
  seriale `sp3 logic selftest flags=0b11111` (tutti e 5 i sub-check OK).

## Perché
SP3 dà al compositor kernel-side un vero **window manager**: le finestre
diventano decorate (riconoscibili, con focus visibile), spostabili,
sovrapponibili con z-order interattivo, e chiudibili — prerequisito per SP4
(compositing SMP-parallelo su `compose_window`) e SP5 (launcher/lifecycle che
riusa `close(id)` e l'invariante di placement `sy >= TITLE_H`).

## Verifica (QEMU+KVM, headless QMP)
ISO `build/comptest.iso` (init `user-bin/compositor-init.sh`), boot headless
`q35 -cpu max accel=kvm:tcg` (RDRAND richiesto dal kernel rng). Driver QMP
`build/wm_verify.py` (mouse PS/2 RELATIVO, cursore virtuale tracciato dal centro
~640,400, REL deltas ≤55px). 4 screendump:
- `wm-before.png`: due finestre decorate (A barra blu/focused, B barra grigia),
  testo bianco "reactor A"/"reactor B", [X] rosso, B sopra A. **OK**.
- `wm-after-drag.png`: drag della barra di A → A trasla a (380,346) seguendo il
  cursore (grab offset corretto, niente salto), B invariata. **OK**.
- `wm-after-raise.png`: raise-from-behind genuino — click barra di B (focus→top,
  `WM-FOCUS 1`), poi click su punto solo-A della surface → A risale sopra B e va a
  fuoco (secondo `WM-FOCUS 1`: `WM-FOCUS` stampa l'**indice z-slot**, e la
  finestra raised+focused finisce sempre all'indice top), A occlude B, barra A
  blu / B grigia. **OK**.
- `wm-after-close.png`: click su [X] di B → B sparisce del tutto (barra+surface),
  **nessun ghost** (present() pulisce il bg ogni frame), resta solo A. **OK**.

Boot-check separato (`make test-boot ISO=build/cmtest.iso`): `TEST_BOOT_PASS` +
`sp3 logic selftest flags=0b11111`.

## File toccati
- CHANGELOG/280-26-06-05-compositor-sp3-window-manager.md (questa entry)
- build/wm_verify.py (driver QMP di verifica runtime — drag/raise/close)
- kernel/src/wasm/wt/wm.rs (SP3: già nei commit 363d1a2 + 420bfd0)
- kernel/src/wasm/wt/mod.rs (wiring boot-check — commit SP3)
- kernel/src/boot/phases/interrupts.rs (marker boot-check — commit SP3)
