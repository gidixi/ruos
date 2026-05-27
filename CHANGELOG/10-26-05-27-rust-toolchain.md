# 10 — Toolchain Rust nightly in WSL + rimozione Toolchain/

**Data:** 2026-05-27

## Cosa
- Installati in WSL Ubuntu: curl, xorriso, e rustup con toolchain nightly +
  componenti rust-src e llvm-tools-preview.
- Verificato il target x86_64-unknown-none (usato via build-std).
- Rimossa la cartella x64barebones/Toolchain/ (cross-gcc ModulePacker, non più usata).

## Perché
Step 1 della roadmap Rust: predisporre il toolchain per il kernel no_std e
liberarsi del cross-gcc.

## File toccati
- x64barebones/Toolchain/ (rimossa)
- CHANGELOG/10-26-05-27-rust-toolchain.md
