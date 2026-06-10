# 408 — mkbinpack: tool host per impacchettare /bin in bin.bgz

**Data:** 2026-06-10

## Cosa
Aggiunto `tools/mkbinpack/`, tool host std che legge N file e li impacchetta in
un container RBIN (`.bgz`) usando `gzip_core::pack::write_archive`. Usage:
`mkbinpack OUT IN...` — usa il basename di ogni IN come nome entry nell'archivio.
Il tool è standalone (propria `[workspace]`) e non fa parte di nessun workspace
esistente.

## Perché
Step 3 del piano gzip-tools / bin-pack-livecd: il Makefile userà `mkbinpack` per
assemblare `bin.bgz` dalla cartella `/bin` al build time, prima che il kernel lo
monti in tmpfs al boot.

## File toccati
- tools/mkbinpack/Cargo.toml
- tools/mkbinpack/src/main.rs
