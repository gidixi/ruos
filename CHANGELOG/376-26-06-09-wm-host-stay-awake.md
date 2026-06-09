# 376 — Host fn wm.stay_awake + wm.wake_on_pty

**Data:** 2026-06-09

## Cosa
Registrate due host fn `wm`: `stay_awake()` (override dinamico: la finestra resta
sveglia il prossimo frame) e `wake_on_pty(idx)` (lega un pair PTY per la sveglia su
output; idx<0 slega).

## Perché
Permettere alle app di forzare l'aggiornamento continuo (monitor, animazioni) e ai
terminali di svegliarsi su output shell mentre dormono.

## File toccati
- kernel/src/wasm/wt/wm.rs
