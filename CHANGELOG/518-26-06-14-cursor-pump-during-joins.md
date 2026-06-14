# 518 — cursore disaccoppiato dal loop: pump durante i join SMP

**Data:** 2026-06-14

## Cosa
Causa radice del "mouse a 1 fps con System Monitor aperto": il cursore software
(sprite disegnato sul framebuffer da `gfx::cursor_move`) avanza SOLO quando gira
`gfx::fold_mouse()`, e `fold_mouse()` è chiamato **una volta per iterazione del
loop del compositor**. Quando un frame è pesante (raster/frame() app), l'iterazione
dura 100-200 ms → il cursore si muove a ~5-10 fps → con render-on-input (517) ad
ogni movimento mouse partiva un render pesante → loop ancora più lento → ~1 fps.
Il sistema di disegno accoppiava la fluidità del puntatore al costo per-frame.

Fix: **pump del cursore durante gli spin-wait dei join SMP.** In `dispatch_frames`,
`dispatch_raster`, `dispatch_bands` il core GUI, mentre aspetta gli AP, gira a vuoto
in `spin_loop()`. Lì ora chiama `fold_mouse()` (throttle 1/4096 spin) tramite il
nuovo `pump_cursor_spin()` → il cursore avanza al rate del mouse (~125 Hz) anche
mentre un frame pesante è in calcolo sugli AP. Il puntatore si **disaccoppia** dal
costo per-frame.

SICUREZZA: durante un join non c'è blit né scrittura della RAM-shadow in volo
(avvengono dopo il join, serialmente sul core GUI), e `fold_mouse`/cursor sono
GUI-core-only sotto `CUR_LOCK` → nessuna race col present. Lo shadow letto da
`cursor_erase` è quello del frame PRECEDENTE (= ciò che è già a schermo, dato che il
blit del nuovo frame avviene dopo il join) → ripristino corretto. Throttle 1/4096
spin (decine di µs ≫ 125 Hz) → niente fame dell'IRQ mouse dal lock-traffic.

## Perché
Le ottimizzazioni precedenti (513-517) hanno tagliato il costo per-frame e cappato
il refresh, ma finché il cursore gira al rate del loop resta scattoso sotto carico.
Disaccoppiarlo è l'unico fix vero. (Alternativa: cursore sull'IRQ mouse → ma corre
col blit su VRAM/shadow → glitch transitori; il pump nei join resta tutto sul core
GUI, serializzato col present, senza race.)

## File toccati
- kernel/src/wasm/wt/wm.rs (fn pump_cursor_spin; usata negli spin-wait dei join di
  dispatch_frames, dispatch_raster, dispatch_bands)
