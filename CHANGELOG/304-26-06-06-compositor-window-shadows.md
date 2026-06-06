# 304 — Ombre delle finestre (drop shadow nel compositor)

**Data:** 2026-06-06

## Cosa

Il compositor kernel-side disegna una **drop shadow** sotto ogni finestra non-bg.

- `compose.rs` guadagna l'**alpha-blend** (prima era painter opaco, alpha ignorato):
  una fase ombra per-finestra, blendata PRIMA della superficie opaca, nell'ordine
  painter (l'ombra cade sulle finestre sotto, mai su quelle sopra).
- Ombra = rettangolo offsettato `(dx=4, dy=6)`, feather `R=10px`, alpha di picco
  `70/255`, falloff **cubico** `(1-d/R)³` da LUT **integer** (concentrata al bordo,
  sfuma in fretta → ombra di contatto definita, non un alone). Salta l'interno
  opaco (coperto dalla surface).
- `WinDesc` guadagna `shadow: bool` (false per il bg). Tutto a interi → il
  composite parallelo SMP resta **bit-identico** al seriale (il test SP4 regge).

## Perché

Profondità/separazione visiva tra finestre e desktop. Compositor-side perché
l'ombra cade su pixel di ALTRE finestre/sfondo: solo il compositor, che conosce
z-order e geometria, può comporla correttamente (l'app non può disegnare fuori
dalla sua superficie).

## File toccati

- kernel/src/wasm/wt/compose.rs
- kernel/src/wasm/wt/wm.rs (campo `WinDesc.shadow` + costruzione in `present`)
