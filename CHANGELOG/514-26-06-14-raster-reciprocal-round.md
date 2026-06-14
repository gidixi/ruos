# 514 — raster: reciproco /255 + round via cast (slow-path per-pixel)

**Data:** 2026-06-14

## Cosa
Secondo giro di ottimizzazione del fill, dopo aver misurato il fix 513 su HW reale:
- **B (System Monitor)**: `r` 274ms → ~120ms (fast-path opaco ha pagato).
- **C (SM + hover menu)**: `r` 491ms → ~479ms (quasi nullo).

C resta alto perché la shell full-screen è dominata dallo **sfondo wallpaper**
(gradiente/immagine = non-flat) → niente fast-path → slow path pieno. Quindi il
residuo è il **costo per-pixel dello slow path**. Attaccato:

1. **`/255.0` → `* INV_255`** (costante `1.0/255.0` a compile-time): le 5 divisioni
   per pixel (fr/fg/fb/fa + `1 - fa/255`) diventano moltiplicazioni (~20 cicli →
   ~4). Differenza ≤1 LSB dal valore esatto (assorbita dal round); **mirrorata in
   gui-core con la stessa costante → il cross-check resta byte-identico**.
2. **`.round()` → `round_nn(x) = (x + 0.5) as i32 as f32`**: i 4 round per pixel
   non usano più il floor SOFTWARE (F32Ext bit-twiddle + branch, no_std non ha
   round nativo) ma un cast troncante hardware. Tutti i valori di blend sono ≥0
   (premoltiplicato, pesi inside ≥0) → `(x+0.5) as i32` == `f32::round` esatto su
   x≥0. Zero deviazione aggiuntiva.

Applicato **identico** a `ruos-raster` (kernel) e `gui-core/raster.rs` (mirror).
Il cross-check ora esercita davvero la nuova aritmetica (sotto `cfg(test)`
`round_nn`/`INV_255` NON sono shadowed da std, a differenza di `.round()` prima).
Verifica host: ruos-raster 13 + `crosscheck` byte-identico verdi; gui-core 44 verdi.

## Perché
Il fast-path 513 copre solo i fill opachi piatti. Wallpaper gradiente/immagine,
grafici animati semi-trasparenti, AA e testo restano slow-path: 5 div + 4 round
software per pixel = ~160 cicli/px dominanti su quei pixel (confermato da C che non
era migliorato). Reciproco + cast li dimezza circa, mantenendo l'invariante reale
(kernel == anteprima PC, garantito dal cross-check).

## File toccati
- ruos-raster/src/lib.rs (INV_255 + round_nn + swap nel blend di raster_tri)
- ruos-desktop/crates/gui-core/src/raster.rs (mirror identico)
