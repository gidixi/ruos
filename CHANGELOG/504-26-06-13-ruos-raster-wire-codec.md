# 504 — ruos-raster: wire codec (encode/decode) + render_wire panic-safe

**Data:** 2026-06-13

## Cosa

Aggiunto a `ruos-raster` il WIRE CODEC: unica fonte di verità del layout byte
app↔kernel della mesh.

- Costanti layout: `VERTEX_WIRE_SIZE = 20`, `INDEX_WIRE_SIZE = 4`,
  `PRIM_WIRE_SIZE = 32` (la spec diceva erroneamente 28 B per Prim; corretto a 32
  = 16 clip + 8 tex_id + 4 idx0 + 4 idx1, con commento in codice).
- Encoder canonici: `encode_verts`, `encode_indices`, `encode_prims` (campi
  little-endian via `to_le_bytes`, packed, no padding, no `transmute`/`repr(C)`).
- Decoder kernel-side: `decode_verts`, `decode_indices`, `decode_prims`
  (`from_le_bytes` su offset fissi, `chunks_exact(N)` → record parziale finale
  scartato, mai panic su buffer corto/garbage).
- `Raster::render_wire(verts, idx, prims, w, h)`: decodifica i 3 buffer poi
  `self.render(...)`. Entry point unico del kernel.
- Panic-safety nel path raster raggiungibile da `render_wire`: guard su range
  `idx0..idx1` (clamp a `idx.len()`) e su valori indice `>= verts.len()` sia in
  `raster_band` sia in `prim_meta`. No-op per input valido (egui non emette mai
  indici fuori range), quindi il cross-check bit-identical vs gui-core resta verde.
- Nuovi test in `src/tests.rs`: `wire_roundtrip_matches_typed_render`,
  `codec_roundtrip_is_identity`, `decode_is_panic_free_on_garbage`.

## Perché

Il kernel inoltra byte controllati dal guest (ruos-window li codifica, il kernel
li decodifica): serve un layout autoritativo byte-per-byte e un confine di
fiducia che NON faccia mai panic su input malformato. Il render dev'essere
host-testabile.

## File toccati

- ruos-raster/src/lib.rs
- ruos-raster/src/tests.rs
- CHANGELOG/504-26-06-13-ruos-raster-wire-codec.md
