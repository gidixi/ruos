# 154 — Task 4+5: sunset deps + listen + accept loop (echo stub)

**Data:** 2026-05-30

## Cosa

### Task 4 — sunset + crypto deps

`kernel/Cargo.toml`:
- `sunset = "0.4"` (no_std SSH RFC 4253)
- `getrandom = "0.2"` features=`["custom"]` per `register_custom_getrandom!`
- Forza software backend RustCrypto su x86_64-unknown-none:

`kernel/.cargo/config.toml` rustflag aggiunti:
```
--cfg curve25519_dalek_backend="serial"
--cfg poly1305_force_soft
--cfg aes_force_soft
--cfg chacha20_force_soft
```

LLVM rifiuta SIMD AVX2 intrinsics su bare-metal → ogni RustCrypto
crate ha proprio flag force-soft. Senza, "rustc-LLVM ERROR: Do not
know how to split the result of this operator".

`kernel/src/ssh/rng_bridge.rs`: `register_custom_getrandom!` →
`crate::rng::fill` (kernel ChaCha20 CSPRNG).

`kernel/src/ssh/sunset_io.rs`: stub `run_session(handle)` — echo
loop tra socket recv/send. Full sunset Runner integration deferred a
Tasks 6-8.

### Task 5 — server.rs accept loop

`crate::ssh::server::spawn()`:
- Load host key + authkeys (Task 2+3)
- `binfo!("ssh listening on 0.0.0.0:22 (task pending start)")`

`crate::executor::run()`:
- Spawn `ssh_serve_task` (nuovo embassy task)
- Task chiama `server::serve_loop_pub()` → `serve_loop()`

`serve_loop()`:
- Loop: alloc TCP socket, listen(22), accept().await
- Su connect: log `client connected`, `run_session(handle).await`,
  log `client disconnected`
- Errori: log + Delay::ticks(100) + retry

## Test

`make run-test` → TEST_PASS. Serial:
```
[T+7.250s] INFO ssh  listening on 0.0.0.0:22 (task pending start)
[T+7.315s] INFO ssh  server ready
[T+7.570s] INFO ssh  accept loop waiting on :22
```

Server attivo. Connection echo manuale verificabile via QEMU hostfwd
(non automated in run-test al momento).

## Cosa NON è ancora

- KEX (key exchange) NON funziona — solo echo socket bytes
- Auth NON valuta pubkey ancora
- Channel exec/pty NON dispatched
- Client OpenSSH connecting → leggerà byte echoed, fallirà protocol

Tasks 6-8 = sunset Runner real event dispatch (~2-3 giorni
aggiuntivi).

## File toccati

- kernel/Cargo.toml (sunset + getrandom)
- kernel/.cargo/config.toml (4 force-soft cfg flags)
- kernel/src/ssh/{rng_bridge,sunset_io,server}.rs
- kernel/src/ssh/mod.rs (`pub mod rng_bridge`)
- kernel/src/executor/mod.rs (`ssh_serve_task`)
- CHANGELOG/154-26-05-30-ssh-sunset-listen-accept.md (questo)
