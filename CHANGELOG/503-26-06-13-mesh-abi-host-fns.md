# 503 — Mesh ABI host fns (wm.commit_mesh / wm.tex_update) + stato raster per-finestra

**Data:** 2026-06-13

## Cosa

Fase 2 del rasterizzatore UI kernel-side (display-server, GPU-less). SOLO store,
nessuna rasterizzazione in questa fase:

- `kernel/Cargo.toml`: aggiunta dipendenza `ruos-raster = { path = "../ruos-raster" }`.
- `WmState` (`kernel/src/wasm/wt/wm.rs`): nuovi campi per-finestra — `raster:
  ruos_raster::Raster` (clear `[0x1e,0x1e,0x1e,0xff]`, come gui-core), `mesh_verts/
  mesh_idx/mesh_prims: Vec<u8>` (buffer wire copiati), `mesh_w/mesh_h: u32`,
  `mesh_dirty: bool` (set da commit_mesh, consumato dal raster step in fase
  successiva), `mesh_mode: bool` (la finestra ha committato almeno una mesh).
  Inizializzati in tutti e tre i costruttori (`run` probe id 0, `worker_app_state`,
  `spawn_named`).
- Nuova host fn `wm.commit_mesh(vp,vl, ip,il, pp,pl, w,h) -> i32`: legge i tre
  buffer (verts/idx/prims) dalla linear memory guest via `wt::mem::read`, li COPIA
  in `WmState`, marca `mesh_dirty + mesh_mode`. Ritorna 0 / 28 su read fault.
- Nuova host fn `wm.tex_update(id:i64, full, x,y,w,h, ptr,len) -> i32`: legge i
  pixel RGBA premoltiplicati e chiama `raster.set_texture(id, pos, w, h, &px)`
  (`pos = None` se `full!=0`, altrimenti `Some((x,y))`). Ritorna 0 / 28. La tex id
  u64 passa come singolo `i64` (il linker accetta i64, cfr. surface_size/window_size).
- `docs/api/wm.md`: aggiunte le entry `commit_mesh` e `tex_update` con il wire
  format (§5 spec), aggiornato "Last reviewed" a 2026-06-13 (26 funzioni).

## Perché

Fase 2 del piano `2026-06-13-ui-kernel-side-raster.md`: la ABI mesh app↔kernel +
lo stato per-finestra dove il kernel terrà mesh/atlante. L'app tessella (egui →
triangoli) e spedisce la mesh; il kernel COPIA in memoria kernel così gli AP del
pool SMP la rasterizzano senza mai toccare la linear memory guest (vincolo
single-accessor / multi-tenant). Qui solo lo store; la rasterizzazione (plan_damage
+ raster_band → surface) e il wiring del compositor sono nelle fasi 3/4.

## File toccati

- kernel/Cargo.toml
- kernel/Cargo.lock
- kernel/src/wasm/wt/wm.rs
- docs/api/wm.md
