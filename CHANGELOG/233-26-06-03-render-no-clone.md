# 233 — perf(console): drop unnecessary per-cell mask clone in render

**Data:** 2026-06-03

## Cosa
Rimosso il clone di `mask.alpha` in `compose_cell` (kernel/src/console/render.rs).
La versione precedente copiava l'intera slice alpha in un `Vec<u8>` locale per
ogni cella dirty sotto l'errata convinzione che il borrow checker richiedesse la
separazione tra `&GlyphMask` (da `cache`) e `&mut Surface`. In realtà i due
parametri `cache: &mut GlyphCache` e `surf: &mut Surface` sono oggetti distinti:
il borrow condiviso `&GlyphMask` (ottenuto da `*cache`) può tranquillamente
coesistere con le chiamate `surf.put_px(&mut *surf)`. Ora `compose_cell` trattiene
direttamente il riferimento `mask` e indicizza `mask.alpha` in loco usando
`mask.w` e `mask.h`. Rimosso anche l'import `alloc::vec::Vec` introdotto solo
per il clone.

## Perché
Il clone produceva ~2 000 heap allocation per ogni flush a schermo pieno, vanificando
l'obiettivo di performance della pipeline di rendering.

## File toccati
- kernel/src/console/render.rs
