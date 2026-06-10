# 402 — Piano implementazione tool gzip/gunzip/zcat

**Data:** 2026-06-10

## Cosa

Piano di implementazione (9 task TDD) per la spec dei tool di compressione:
scaffold `user/gzip-core`, CRC32, compress, decompress, CLI, tre bin thin,
wiring Makefile `BIN_TOOLS`, smoke roundtrip in `smoke.sh` + assert
`TEST_FAIL_GZIP` in `make run-test`, changelog finale.

## Perché

Step successivo del ciclo spec → piano → implementazione per i tool gzip
(spec: docs/superpowers/specs/2026-06-10-gzip-tools-design.md, changelog 401).

## File toccati

- docs/superpowers/plans/2026-06-10-gzip-tools.md
