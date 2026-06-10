# 01 — gzip-core: decompress()

**Data:** 2026-06-10

## Cosa
Aggiunto `decompress()` in `gzip-core` con parsing header gzip RFC 1952 (10 byte
fissi + FEXTRA/FNAME/FCOMMENT/FHCRC opzionali), inflate raw via
`miniz_oxide::inflate::core::decompress` con conteggio esatto dei byte consumati,
verifica CRC32 e ISIZE nel trailer, rifiuto di trailing garbage / multi-member.
Aggiornato `lib.rs` per esportare `decompress`. Aggiunto golden test con bytes reali
da `printf 'hello ruos' | gzip -n -6` su Ubuntu 22.04 + 10 test TDD (roundtrip,
edge case, error path).

## Perché
Task 4 del piano gzip-core: implementare decompress con TDD (red → green). Serve
ai tool `gunzip`/`zcat` del userland ruos.

## File toccati
- user/gzip-core/src/format.rs
- user/gzip-core/src/lib.rs
- CHANGELOG/01-26-06-10-gzip-core-decompress.md
