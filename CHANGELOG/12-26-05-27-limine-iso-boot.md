# 12 — Limine ISO + Makefile + boot seriale "hello"

**Data:** 2026-05-27

## Cosa
- limine.conf (protocollo Limine, kernel /boot/kernel, sintassi Limine v8+).
- Makefile orchestratore: build cargo, clone+build Limine binario, assemblaggio
  iso_root, xorriso (BIOS El Torito) + limine bios-install, run/run-test/clean.
- Limine bootloader vendorizzato da branch binario **v11.x-binary** (tag risolto
  v11.4.1-binary): e' la versione che supporta la base revision 6 richiesta dal
  crate limine 0.6.3 (BaseRevision::new() => revision 6). Con v8.7.0/v9.6.7 il
  kernel bootava ma BaseRevision::is_supported() era false; con v11.4.1 e' true.
- Makefile: aggiunto SHELL := /bin/bash perche' la recipe usa `source` (non
  disponibile in /bin/sh).
- Boot headless in QEMU verifica la stringa seriale "MinimalOS-rs: hello serial".
- Pinnato il nightly esatto in rust-toolchain.toml (nightly-2026-05-26) per
  riproducibilita'.

## Perché
Completa il milestone "ora sono in Rust": kernel Rust che bota da Limine e parla
sulla seriale, primo artefatto eseguibile della riscrittura.

## File toccati
- limine.conf
- Makefile
- kernel/rust-toolchain.toml
- CHANGELOG/12-26-05-27-limine-iso-boot.md
