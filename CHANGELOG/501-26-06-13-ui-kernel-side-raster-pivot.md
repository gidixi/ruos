# 501 — Pivot architettura: raster UI kernel-side (display-server)

**Data:** 2026-06-13

## Cosa
Confermata da Giuseppe l'**Opzione C**: spostare la rasterizzazione UI dal wasm
dell'app al **kernel**, parallela sul pool SMP esistente, niente thread per-app,
niente regressioni. Nuovi documenti:
- Spec `docs/superpowers/specs/2026-06-13-ui-kernel-side-raster-design.md` (autoritativa):
  flusso (app tessella → `wm.commit_mesh` wire → kernel copia → `plan_damage` +
  `dispatch_raster` bande SMP → surface per-finestra → compositor INVARIATO),
  componenti, ABI mesh, crate `ruos-raster` no_std (port di `gui-core/raster.rs`),
  regression-safety (path pixel doppio in migrazione + cross-check bit-identico vs
  gui-core + A/B per app), rischi+mitigazioni (copia mesh in kernel-mem → no
  guest-mem dagli AP; edge f64; determinismo nativo no_std), wire format.
- Piano `docs/superpowers/plans/2026-06-13-ui-kernel-side-raster.md`: 5 fasi
  (ruos-raster+cross-check → ABI → raster kernel seriale+A/B → SMP → migrazione) +
  Leva #0 repaint scheduling indipendente.
- Vecchia spec `2026-06-13-ui-parallel-raster-design.md` marcata **SUPERSEDED**
  (approccio in-wasm A/D scartato: le app sono `wasip1` single-thread).

## Perché
Le app finestra sono `wasm32-wasip1` (1 thread): parallelizzare il raster DENTRO
l'app richiedeva convertirle tutte a `wasip1-threads` (overhead per-finestra +
rischio regressioni). C centralizza il raster nel kernel riusando il pool SMP già
provato (lo stesso del compositing), senza toccare le app oltre `ruos-window`, e con
garanzia bit-identica (il refactor gui-core band-able diventa il golden reference).

## File toccati
- docs/superpowers/specs/2026-06-13-ui-kernel-side-raster-design.md (nuovo)
- docs/superpowers/specs/2026-06-13-ui-parallel-raster-design.md (superseded note)
- docs/superpowers/plans/2026-06-13-ui-kernel-side-raster.md (nuovo)
- CHANGELOG/501-26-06-13-ui-kernel-side-raster-pivot.md (nuovo)
