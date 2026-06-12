# 483 — Overlay FPS on-screen del compositor (feature wm-fps)

**Data:** 2026-06-12

## Cosa

Nuova feature kernel `wm-fps`: telemetria framerate/timing del compositor.

- Nel run loop: contatori per finestra ~1 s — present/s (fps visibili),
  iter/s del loop, durata avg/max di `frame_all` e avg di `present` (TSC →
  µs via `tsc_per_ms`). Log `binfo!("wmfps", ...)` ogni secondo; la finestra
  di warm-up (primi 90 frame) è scartata (il primo frame egui ~1s drogherebbe
  la media).
- **Overlay on-screen** in basso a destra (visibile su VBox/HW dove il log
  seriale non c'è): box 2 righe disegnato direttamente sul framebuffer ogni
  iter — `display: N fps (M Hz)` + `rendering: X ms blit: Y ms`.
- `build-iso.ps1`: prompt interattivo default-YES per attivare `wm-fps`
  (switch `-Fps`/`-NoFps` per forzare senza prompt) + **fix**: le feature
  multiple ora arrivano a cargo separate da virgola (prima lo spazio
  spezzava gli argomenti → "unexpected argument").

Misura con la feature attiva (QEMU TCG, 2 finestre reactor): `frame_all`
parallelo ~1.4 ms vs seriale ~2.1 ms — il dispatch parallelo di Fase 1 è
più veloce, non più lento.

## Perché

Dopo il compositor parallelo (changelog 476) serviva un modo per misurare il
framerate effettivo invece di giudicare a occhio ("sembra meno fluido").
Zero overhead di default: tutto gated compile-time dietro `wm-fps`.

## File toccati

- kernel/src/wasm/wt/wm.rs (telemetria + overlay nel run loop)
- kernel/Cargo.toml (feature wm-fps)
- build-iso.ps1 (prompt FPS default-Y, -Fps/-NoFps, fix feature multiple)
