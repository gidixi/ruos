# 375 — Gating sleep delle frame()

**Data:** 2026-06-09

## Cosa
`should_wake` (puro) + `Compositor::compute_awake`; `frame_all` salta `frame()`
per le finestre dormienti; il run loop bumpa `frame_no` e azzera
`stay_awake_request` prima di ogni `frame_all`. Marcatura attività su input.

## Perché
Le app idle non devono consumare CPU (esecuzione WASM ogni frame). Dormono e si
svegliano su eventi/override/dati PTY.

## File toccati
- kernel/src/wasm/wt/wm.rs
