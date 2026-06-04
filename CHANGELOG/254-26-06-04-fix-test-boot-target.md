# 254 — Fix make test-boot (kernel stale + RDRAND panic)

**Data:** 2026-06-04

## Cosa
Due fix chirurgici al target `test-boot` del Makefile:
- aggiunto `--release` alla `cargo build`: prima buildava in debug ma poi copiava
  `$(KERNEL)` (path release), staggiando un kernel STALE — i boot-check non
  giravano mai col codice/feature appena compilati;
- aggiunto `-machine q35 -cpu max -device qemu-xhci` al QEMU: senza `-cpu max` la
  CPU emulata non ha RDRAND e il boot panica in `rng.rs` ("CPU lacks RDRAND").

## Perché
Verificando i boot-check (mouse, exec W^X, wasmtime) `make test-boot` falliva per
questi due motivi non legati al codice. Restano problemi pre-esistenti separati
(test-boot grep "smoke" ma usa init di default e nessun disco) non affrontati qui.

## File toccati
- Makefile
