# 414 — Makefile: pack bin.bgz nell'ISO + gate unpack_bin

**Data:** 2026-06-10

## Cosa
`make iso` ora builda il tool host `mkbinpack`, stage l'intero `/bin` in
`build/binstage`, lo impacchetta in `iso_root/bin.bgz` (container RBIN) e ship un
set rescue loose in `iso_root/rescue` (shell ls cat echo dmesg lspci). Niente più
`iso_root/bin/` loose. Asserzioni `run-test`/`run-test-usb` aggiornate: al posto di
`/bin overlaid from ISO9660` / `/bin overlaid from USB-MSC` ora verificano
`unpacked N bins from bin.bgz`.

## Perché
Completa la pipeline build per il live-CD via `bin.bgz`: l'archivio compresso è
caricato da Limine e decompresso dalla fase kernel `unpack_bin`. `build-iso.ps1`
resta invariato (chiama `make iso`). Verificato su hardware reale (boot + /bin
popolato, USB scollegabile).

## File toccati
- Makefile
- tools/mkbinpack/Cargo.lock
- CHANGELOG/414-26-06-10-makefile-bin-bgz-pack.md
