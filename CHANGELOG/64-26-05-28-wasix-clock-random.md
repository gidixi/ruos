# 64 — WASIX clock_time_get + random_get + stdin from kbd queue

**Data:** 2026-05-28

## Cosa

- Nuovo modulo `kernel/src/wasm/host/clock.rs`: implementa `clock_time_get` e
  `clock_res_get` usando il contatore TICKS a 100 Hz (10 ms per tick → 10^7 ns).
- Nuovo modulo `kernel/src/wasm/host/random.rs`: implementa `random_get` con un
  PRNG xorshift a 64 bit seedato da TICKS. Non crypto-safe; verrà sostituito con
  RDRAND al Task 14 (SSH).
- `kernel/src/wasm/state.rs`: aggiunto `FdEntry::Stdin`; FD 0 inizializzato a
  `Some(FdEntry::Stdin)` invece di `None`.
- `kernel/src/wasm/host/fd.rs`: `fd_read` gestisce `FdEntry::Stdin` leggendo 1
  byte dalla keyboard async queue via `embassy_futures::block_on`.
- `kernel/src/wasm/host/mod.rs`: espone e collega i moduli `clock` e `random`.
- `user/init/Cargo.toml`: aggiunta dipendenza `getrandom = "0.2"`.
- `user/init/src/main.rs`: stampa uptime_ms e 16 byte random in esadecimale;
  emette il sentinel `init.wasm: clock_rand ok`.
- `Makefile`: `HELLO` aggiornato a `init.wasm: clock_rand ok`.

## Perché

Task 4 del bootstrap WASIX (Step 10). `SystemTime::now()` in Rust chiama
`clock_time_get`; `getrandom` chiama `random_get`. Servono entrambi per
applicazioni WASM realistiche. FD 0 è necessario per la futura shell interattiva.

## File toccati

- `kernel/src/wasm/host/clock.rs` (nuovo)
- `kernel/src/wasm/host/random.rs` (nuovo)
- `kernel/src/wasm/host/mod.rs`
- `kernel/src/wasm/host/fd.rs`
- `kernel/src/wasm/state.rs`
- `user/init/Cargo.toml`
- `user/init/src/main.rs`
- `user-bin/init.wasm`
- `Makefile`
- `CHANGELOG/64-26-05-28-wasix-clock-random.md`
