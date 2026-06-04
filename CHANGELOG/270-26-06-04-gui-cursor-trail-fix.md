# 270 — Fix: scia del cursore mouse (frecce residue muovendo piano)

**Data:** 2026-06-04

## Cosa
Risolta la scia del puntatore: muovendo il mouse **piano** restavano frecce
residue sullo schermo (veloce no). Ora nessuna scia, a qualsiasi velocità.

## Root cause
Bug nel cursore software del kernel (`crate::gfx`), **smascherato dal dirty-rect**
(CHANGELOG 269). `blit()` ricomponeva il cursore con `cursor_after_blit()`, che
**salvava il framebuffer sotto il cursore DOPO che lo sprite era già disegnato**,
senza prima cancellarlo. Salvava quindi lo **sprite stesso** come "sfondo".

Prima del dirty-rect i blit erano full-screen e coprivano SEMPRE l'area del
cursore: `cursor_after_blit` salvava i pixel egui freschi (sfondo corretto), e il
bug era mascherato. Col dirty-rect i blit sono parziali e spesso NON toccano il
cursore → il framebuffer lì conteneva ancora lo sprite → veniva salvato come
sfondo. La mossa successiva (`cursor_move` → `cursor_erase`) "ripristinava" lo
sprite alla vecchia posizione = freccia residua.

Veloce vs piano: `fold_mouse` coalizza i pacchetti PS/2 in un solo spostamento
per frame, quindi veloce = salti grandi e radi (scia invisibile); piano = molti
passi piccoli sovrapposti = scia densa visibile.

## Fix
`blit()` ora **solleva il cursore PRIMA di scrivere** (`cursor_erase()` →
ripristina lo sfondo vero, sprite rimosso) e lo **ridipinge dopo**
(`cursor_repaint()` = salva-sfondo + disegna). Lo sfondo salvato è sempre il
framebuffer reale, mai lo sprite → `cursor_erase` ripristina sempre pixel veri →
nessuna scia. `cursor_after_blit` (paint-only con invalidate) sostituito da
`cursor_repaint` (l'invalidate non serve più: l'erase iniziale gestisce la
validità). `cursor_move` (mouse) invariato e si compone correttamente. Fuori da
GUI mode entrambe le chiamate sono no-op (boot self-test del blit invariato).

## Verifica
Confermato dall'utente su VirtualBox: scia sparita a ogni velocità. Kernel
compila, `os.iso` ribuilt.

## File toccati
- kernel/src/gfx/mod.rs (blit: cursor_erase prima della scrittura; cursor_after_blit → cursor_repaint)
