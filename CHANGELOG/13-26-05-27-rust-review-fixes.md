# 13 — Review fix milestone Rust serial-hello

**Data:** 2026-05-27

## Cosa

Correzioni dalle code-review del milestone:
- `kernel/src/main.rs`: inizializza la seriale **prima** del check base-revision
  Limine, così un fallimento è osservabile sulla seriale (commit b05b6aa).
- `Makefile`: target `run-test` ora self-validating (grep della stringa hello,
  exit non-zero se manca → `HELLO` non più variabile morta); pin esatto del tag
  bootloader `v11.4.1-binary` (riproducibilità); commento sul requisito bash
  (commit e039ede).
- `x64barebones/Readme.txt`: nota che l'albero C è solo riferimento post-pivot e
  non più buildabile (Toolchain rimossa, Image/Makefile dipende da ModulePacker).

## Perché

Chiudere i rilievi minor/important delle review prima del merge, lasciando build
riproducibile, test self-validating e albero di riferimento coerente.

## File toccati

- kernel/src/main.rs
- Makefile
- x64barebones/Readme.txt
- CHANGELOG/13-26-05-27-rust-review-fixes.md
