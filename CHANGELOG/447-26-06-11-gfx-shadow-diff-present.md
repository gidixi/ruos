# 447 — gfx: shadow RAM + present diff-based, cursore senza letture VRAM

**Data:** 2026-06-11

## Cosa

`gfx`: nuovo shadow buffer in RAM (RGBA8888, ultima frame presentata, alloc
una tantum in `init()` se panel 32-bpp):

1. **Diff present** — `blit` confronta (memcmp) ogni riga in arrivo con lo
   shadow e salta la scrittura VRAM quando identica. `FORCE_FULL` (settato a
   `init()`/`enter()`, azzerato dal primo blit full-screen) copre il caso
   "schermo con contenuto ignoto" (testo console dopo `enter`).
2. **Cursore software senza letture FB** — lo sprite vive solo sul
   framebuffer; lo shadow è sempre lo sfondo vero. `cursor_erase` ripristina
   dallo shadow, `cursor_paint` è write-only. `CUR_SAVE` (che leggeva ~230 px
   dalla VRAM a ogni repaint/move) rimosso; eliminato anche il pre-erase nel
   blit (non più necessario: lo sfondo non va mai "salvato"). Nuovo `CUR_LOCK`
   serializza le sequenze erase/paint tra core.

Panel non-32bpp: shadow assente → blit scrive sempre (come prima), cursore
disabilitato (prima scriveva u32 su pixel da 3 byte: corruzione).

## Perché

Su hardware reale il framebuffer è VRAM dietro PCIe: anche con mapping WC
(changelog 446) le letture restano uncached (~µs l'una) e le scritture hanno
banda finita. Il present full-screen del compositor (~8 MB/frame a 1080p)
riscriveva tutto anche a schermo fermo; il cursore leggeva la VRAM a ogni
mouse move. Col diff le righe immutate costano solo un memcmp in RAM e il
cursore non legge mai la VRAM. In VM nessuna differenza percepibile (FB = RAM
host).

## File toccati

- kernel/src/gfx/mod.rs
