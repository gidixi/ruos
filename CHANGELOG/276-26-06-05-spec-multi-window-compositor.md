# 276 — Spec: desktop multi-finestra a processi separati (compositor kernel-side)

**Data:** 2026-06-05

## Cosa
Design doc per un desktop dove ogni app è un **processo wasm separato** con la sua
finestra, e un **compositor nel kernel** (Rust no_std) compone le finestre +
instrada l'input. Spec:
`docs/superpowers/specs/2026-06-05-multi-window-compositor-design.md`.

## Decisioni (dal brainstorming)
- App = processi separati (isolamento sandbox), NON in-process.
- Compositor **kernel-side** (possiede framebuffer + input + alloca finestre).
- Concorrenza = **reactor cooperativo sul BSP**: l'app esporta `frame()`, il kernel
  guida il loop e chiama `frame()` di ogni app a turno (niente fiber). Le surface
  sono buffer dell'app, `commit(ptr,len)` → il kernel legge.
- **Multi-CPU per il LAVORO** (compositing/raster paralleli sul compute-pool SMP
  Fase-2 + offload per-app), NON un core per app.

## Prossimo
**GATE** (primo sotto-progetto): 2 app reactor mini, 2 surface, compositing
affiancato, entrambe si aggiornano. De-rischia: multi-istanza wasm persistente +
`frame()` round-robin + commit/read surface. Poi: input/focus → WM → compositing
SMP → launcher.

## File toccati
- docs/superpowers/specs/2026-06-05-multi-window-compositor-design.md
