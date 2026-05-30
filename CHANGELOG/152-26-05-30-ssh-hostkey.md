# 152 — Task 2: SSH Ed25519 host key load/generate

**Data:** 2026-05-30

## Cosa

`kernel/src/ssh/hostkey.rs`: real impl.
- `HostKey { signing: SigningKey }` + `public()` -> [u8; 32]
- `load_or_generate(path)`:
  1. try `vfs::open(path, READ)` + `read 32 bytes`
  2. if absent, `rng::fill(&mut seed)` (kernel ChaCha20 CSPRNG) +
     `vfs::open(path, CREATE|WRITE|TRUNCATE)` + `write seed`
- Format: raw 32-byte seed (no PEM, no PKCS8). Public key re-derived
  from seed each boot via ed25519-dalek `SigningKey::from_bytes`.

### Build infra

- `kernel/Cargo.toml`: `ed25519-dalek = "2"` + `sha2 = "0.10"`
  (entrambi `default-features = false`, sha2 con `force-soft` perché
  cpufeatures AVX detect fallisce su x86_64-unknown-none).
- `kernel/.cargo/config.toml` rustflag:
  `--cfg curve25519_dalek_backend="serial"` per forzare backend
  software (SIMD AVX2 backend = LLVM "Do not know how to split"
  error su target bare-metal).

### Path scelti

`CONFIG.host_key_path = "/mnt/host.key"` (8.3 short-name limit:
"ssh_host_key" troncherebbe a "SSH_HOST" rompendo persistence).
`authkeys_path = "/mnt/auth.key"`.

### Spawn logging

`boot/phases/userland.rs`: pattern match risultato `ssh::spawn()`,
log `Ok` come binfo, `Err(e)` come bwarn — diagnostic chiaro.

## Test

`make run-test`:
- Boot 1: `host key generated at /mnt/host.key`, fingerprint stabile
- Boot 2: `host key loaded (32 bytes seed)`, **stesso fingerprint**
- Persistence on FAT verified

mtools post-test:
```
HOST     KEY        32 ...
```
32 byte = raw seed Ed25519.

## File toccati

- kernel/src/ssh/{mod,hostkey,server}.rs
- kernel/Cargo.toml (ed25519-dalek + sha2)
- kernel/.cargo/config.toml (curve25519_dalek_backend serial)
- kernel/src/boot/phases/userland.rs (logging)
- CHANGELOG/152-26-05-30-ssh-hostkey.md (questo)
