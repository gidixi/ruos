# 405 — Piano: live-CD /bin via bin.bgz

**Data:** 2026-06-10

## Cosa
Piano di implementazione (9 task, TDD + commit frequenti) per la spec 404:
gzip-core no_std + modulo `pack` (container RBIN), tool host `mkbinpack`, fase
boot kernel `unpack_bin` (bin.bgz → tmpfs /bin) con rescue fallback, rimozione
`media_bin`/`bin_overlay_task`, limine.conf `/archive/`+`/rescue/`, Makefile pack
+ asserzioni gate, verifica peak RAM.

## Perché
Tradurre la spec approvata in passi bite-sized eseguibili da subagent/inline.

## File toccati
- docs/superpowers/plans/2026-06-10-bin-pack-livecd.md
- CHANGELOG/405-26-06-10-bin-pack-livecd-plan.md
