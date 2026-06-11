# 444 — `#[inline]` su check_bounds (boundary memoria guest wasmi)

**Data:** 2026-06-11

## Cosa
Aggiunto `#[inline]` a `check_bounds` in `kernel/src/wasm/host/mem.rs`, il
controllo di bound attraversato da ogni host fn wasmi che legge/scrive la memoria
guest (`guest_read`, `guest_read_into`, `guest_write`, `guest_write_u32`).

## Perché
La funzione è piccola e `pub(crate)`, quindi chiamata cross-module (potenzialmente
cross-codegen-unit): senza il suggerimento l'inline non è garantito. `#[inline]`
fonde il check col sito chiamante eliminando l'overhead della call. Scelto
`#[inline]` e non `#[inline(always)]` per lasciare a LLVM la decisione sui cold
path ed evitare gonfiore della i-cache.

Valutate e scartate le altre micro-ottimizzazioni proposte:
- `core::intrinsics::unlikely` sui rami d'errore: guadagno trascurabile rispetto
  al costo dominante (alloc heap per chiamata in `guest_read` + check interno di
  wasmi in `mem.read`).
- aritmetica branchless / maschere bitwise: inapplicabile, `size = n_pagine *
  65536` non è potenza di 2.
- validazione a blocchi: già implementata (l'intero range `ptr+len` è validato in
  un colpo prima di ogni accesso).

## File toccati
- kernel/src/wasm/host/mem.rs
- CHANGELOG/444-26-06-11-check-bounds-inline.md
