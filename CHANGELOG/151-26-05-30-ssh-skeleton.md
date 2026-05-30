# 151 — Task 1: SSH module skeleton + boot wire

**Data:** 2026-05-30

## Cosa

5 file placeholder + Config + SshError + boot hook:

- `kernel/src/ssh/mod.rs`: `Config { port, host_key_path, authkeys_path }`,
  `SshError { NotImplemented, VfsIo, NoNetwork, NoStorage, BadAuthKey,
  Crypto }` + `Display`, `spawn() -> Result<(), SshError>`
- `kernel/src/ssh/hostkey.rs`: stub `load_or_generate`
- `kernel/src/ssh/authkeys.rs`: stub `load`
- `kernel/src/ssh/server.rs`: stub `spawn` logs intent
- `kernel/src/ssh/channel.rs`: empty placeholder
- `kernel/src/ssh/sunset_io.rs`: empty placeholder
- `mod ssh;` in main.rs
- `boot/phases/userland.rs`: chiama `crate::ssh::spawn()` after net::init,
  non-fatal (ignora errno)

## Test

`make run-test` → TEST_PASS. Serial:
```
[T+6.058s] WARN ssh spawn skeleton (port 22 host_key=/mnt/etc/ssh/host_key
                authkeys=/mnt/etc/ssh/authorized_keys) — pending Tasks 2-8
```

## File toccati

- kernel/src/ssh/{mod,hostkey,authkeys,server,channel,sunset_io}.rs (nuovi)
- kernel/src/main.rs (`mod ssh;`)
- kernel/src/boot/phases/userland.rs (call `ssh::spawn`)
- CHANGELOG/151-26-05-30-ssh-skeleton.md (questo)
