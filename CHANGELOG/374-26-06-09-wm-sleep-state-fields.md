# 374 — Campi stato sleep nel compositor

**Data:** 2026-06-09

## Cosa
`WmState`: `stay_awake_request`, `wake_pty`. `Window`: `awake`, `last_active_frame`,
`framed_once`. `Compositor`: `frame_no`. Inizializzati in tutti i costruttori.

## Perché
Stato base per lo sleep/wake cooperativo delle finestre (gating di frame()).

## File toccati
- kernel/src/wasm/wt/wm.rs
