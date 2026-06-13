# 505 — Raster kernel-side della mesh (render_wire → surface finestra) + mesh-mode shell/terminal

**Data:** 2026-06-13

## Cosa

Chiuso il path end-to-end "tessella nell'app → rasterizza nel kernel" (spec
`2026-06-13-ui-kernel-side-raster-design.md`, Fase 3):

- **`ruos-window` (submodule, commit `b7ad167`):** nuovo `enable_mesh_mode()` +
  flag `MESH_MODE`. In `frame_once`/`frame_once_bare`, dopo `ctx.tessellate`, se
  mesh-mode è attivo si chiama `ship_mesh(...)` invece del raster locale
  (tiny-skia) + `wm.commit`. `ship_mesh` codifica `out.textures_delta` →
  `wm.tex_update` (atlante prima, così il kernel l'ha pronto) e i
  `ClippedPrimitive` → tre buffer nel WIRE format autorevole di `ruos-raster`
  (Vertex 20 B, Index 4 B u32, Prim 32 B) → `wm.commit_mesh`. Solo
  `Primitive::Mesh` (le `Callback` GPU si saltano). Aggiunti gli extern
  `commit_mesh`/`tex_update` al blocco `#[link(wasm_import_module="wm")]`.
- **shell + terminal-app:** chiamano `enable_mesh_mode()` una volta all'avvio
  (la shell bg renderizza ogni frame headless → esercita il path al boot; il
  terminal è il target A/B del piano). Le altre app restano sul path pixel.
- **kernel `wm.rs`:** nuovo `Compositor::raster_meshes()` chiamato in `run()`
  subito dopo `frame_all()` e prima del present. Per ogni finestra sveglia in
  mesh-mode con mesh nuova (`mesh_dirty`), `raster.render_wire(...)` rasterizza la
  mesh kernel-side nella `pixels` surface della finestra (seriale per ora; lo
  split SMP a bande è una fase successiva). `compose_window` e il path
  `wm.commit` (pixel) restano INVARIATI: una finestra mesh-mode appare identica al
  compositor.

## Perché

Spostare il raster pesante fuori da `frame()` dell'app verso il kernel (che lo
parallelizzerà sul pool SMP esistente), senza dare thread alle app e senza
regressioni. Il path doppio (pixel `wm.commit` legacy + mesh `wm.commit_mesh`)
permette la migrazione una app alla volta.

## Verifica

`make iso CARGO_FEATURES=wm-fps` → compila. Boot headless QEMU (q35, smp 4):
`ruos boot OK` presente, `mesh render win=0 1280x800` continuo (shell via il path
mesh kernel-side), `composite cores=4 [0,1,2,3]` presente, ZERO panic/PANIC, ZERO
WATCHDOG.

## File toccati

- ruos-desktop/crates/ruos-window/src/lib.rs (submodule)
- ruos-desktop/apps/shell/src/lib.rs (submodule)
- ruos-desktop/apps/terminal-app/src/lib.rs (submodule)
- kernel/src/wasm/wt/wm.rs
- CHANGELOG/505-26-06-13-kernel-side-mesh-raster.md
