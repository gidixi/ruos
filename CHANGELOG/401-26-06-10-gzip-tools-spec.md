# 401 — Spec tool compressione gzip/gunzip/zcat

**Data:** 2026-06-10

## Cosa

Spec di design per i tre tool userland `gzip`, `gunzip`, `zcat`
(wasm32-wasip1, wasmi): formato gzip RFC 1952 via `miniz_oxide` pure Rust,
lib condivisa `user/gzip-core` + tre bin thin, semantica Unix classica
(`gzip f` → `f.gz` + delete, flag `-c -k -d -1..-9`), stdin→stdout senza
argomenti.

## Perché

ruos non ha alcun sistema di compressione; gzip è il formato più diffuso e
miniz_oxide è l'unica via pure-Rust completa (compress+decompress) che
compila su wasm32-wasip1 senza attriti.

## File toccati

- docs/superpowers/specs/2026-06-10-gzip-tools-design.md
