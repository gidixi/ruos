# 100 — framebuffer console: anti-aliasing del font

**Data:** 2026-05-30

## Cosa
`draw_glyph` e `self_test` ora fanno blending lineare per canale
`fg*α + bg*(1-α)` con α = intensity/255, invece del threshold a 1 bit
(`intensity >= 128`). Aggiunta funzione locale `blend()` in `fb.rs`.

## Perché
Il crate `noto-sans-mono-bitmap` fornisce raster grayscale con
anti-aliasing pre-calcolato (8 bit di intensità per pixel). Il threshold
secco lo collassava a 1 bit, producendo bordi scalettati molto visibili
in VirtualBox e su monitor ad alta densità. Il blend lineare preserva
l'AA del font, qualità visiva dei glifi nettamente migliore senza
cambiare font, dimensione o risoluzione.

`make run-test` continua a passare: il `self_test` confronta i pixel
attesi calcolati con la stessa `blend()` usata in `draw_glyph`, quindi
l'asserzione resta consistente.

## File toccati
- kernel/src/console/fb.rs
