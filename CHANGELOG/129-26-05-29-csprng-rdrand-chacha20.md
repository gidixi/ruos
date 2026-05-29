# 129 — CSPRNG RDRAND→ChaCha20; random_get rewire

**Data:** 2026-05-29

## Cosa
`rng.rs`: ChaCha20Rng seedato da RDRAND (CPUID check; fatale se assente),
`fill`/`next_u64`/`init`. `random_get` ora usa `rng::fill`. `rng::init()` a boot
prima di `net::init()`. Aggiunto `-cpu max` a QEMU (run e run-test) per esporre
RDRAND (q35 default non lo include).

## Perché
Entropia sicura per WASI random_get e (futuro) SSH; mai timer come seed.

## File toccati
- kernel/src/rng.rs
- kernel/src/main.rs
- kernel/src/wasm/host/random.rs
- kernel/src/boot/phases/userland.rs
- Makefile
