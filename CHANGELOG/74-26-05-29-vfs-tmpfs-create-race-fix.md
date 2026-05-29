# 74 — Fix race walk+insert in tmpfs::open CREATE

**Data:** 2026-05-29

## Cosa

`kernel/src/vfs/tmpfs.rs::Tmpfs::open` riscritto:

1. Fast path non-CREATE: tenta `walk` + restituisce o errore.
2. CREATE path: lock parent **prima**, double-check children sotto lock,
   poi insert. Se altra fiber ha già inserito tra il fast-path walk e
   l'acquisizione del lock, adotta il loro inode.

Eliminato il commento obsoleto che diceva "Safe today because all VFS
futures complete on a single poll under block_on". Con Step 10.5 fiber
cooperative async, due fiber possono concorrenza-mente fare open(CREATE)
sullo stesso path.

## Perché

TOCTOU bug latente diventato reachable da Step 10.5. Pattern check-then-act
sotto due lock distinti: walk fallisce con `NotFound`, fiber B inserisce
`name`, fiber A fa `parent.lock() + insert` sovrascrivendo B. B perde
silenziosamente il riferimento al suo inode.

Double-check sotto unico lock = atomicity. Costo: leggera duplicazione
del match `TmpKind`. Tollerabile.

## File toccati

- kernel/src/vfs/tmpfs.rs
- CHANGELOG/74-26-05-29-vfs-tmpfs-create-race-fix.md (nuovo)
