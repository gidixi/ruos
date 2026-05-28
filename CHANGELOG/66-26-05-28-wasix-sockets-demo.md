# 66 — sock_* host fns + server/client demo (Step 10 Task 6)

**Data:** 2026-05-28

## Cosa

- `kernel/src/net/sockets.rs`: `SockPool` (POOL static), `listen`, `connect_sync`,
  `accept_sync`, `recv_sync`, `send_sync` — synchronous smoltcp wrappers that
  spin-poll `net::poll()` directly so they work from inside synchronous host fns.
- `kernel/src/wasm/host/sock.rs` (nuovo): `sock_accept` host fn
  (`wasi_snapshot_preview1::sock_accept`). Calls `accept_sync` then writes the
  accepted FD (same as the listen FD — single-connection model).
- `kernel/src/wasm/state.rs`: aggiunto `FdEntry::Socket(usize)`.
- `kernel/src/wasm/host/fd.rs`: dispatch socket in `fd_read`/`fd_write` via
  `recv_sync`/`send_sync`.
- `kernel/src/wasm/host/mod.rs`: esposto il modulo `sock`, chiama `sock::link`.
- `kernel/src/wasm/mod.rs`: `SERVER_SOCK_IDX`/`CLIENT_SOCK_IDX` statics;
  `setup_demo_sockets()` che pre-alloca, connette, e pre-carica "pong" nel buffer
  RX del client prima che l'executor parta; `run_at` inietta il FD 4 come socket.
- `kernel/src/main.rs`: chiama `wasm::setup_demo_sockets()` dopo `net::init()`.
- `kernel/src/executor/mod.rs`: spawna `wasm_task("/server.wasm")` e
  `wasm_task("/client.wasm")` (pool_size già 3).
- `user/server/` (nuovo): crate `server`, build target `wasm32-wasip1`.
  Usa `sock_accept(4, 0, &fd)` poi `fd_read`/`fd_write` raw via
  `#[link(wasm_import_module="wasi_snapshot_preview1")]`.
- `user/client/` (nuovo): crate `client`, build target `wasm32-wasip1`.
  Usa `fd_write`/`fd_read` raw sullo stesso modulo.
- `user/Cargo.toml`: members += `server`, `client`.
- `limine.conf`: dichiara 3 moduli (`init.wasm`, `server.wasm`, `client.wasm`).
- `Makefile`: regole `user-bin/server.wasm`, `user-bin/client.wasm`; iso dipende da
  tutti e 3; HELLO → `client.wasm: rx='pong'`.

## Perché

Sesto e ultimo task dello Step 10 (WASIX bootstrap).

La wasm32-wasip1 stdlib non supporta `TcpListener::bind` né `TcpStream::connect`
(entrambi restituiscono `unsupported()`). Il modello WASI Preview 1 prevede invece
socket activation: il runtime crea e connette i socket, poi li passa al modulo come
preopen FD. Il kernel pre-alloca i due socket smoltcp, esegue la 3-way handshake
(con `connect_sync`), e pre-carica "pong" nel buffer RX del client prima che
l'executor parta — così i wasm task possono girare in qualsiasi ordine senza
deadlock cooperativo.

## File toccati

- `kernel/src/net/sockets.rs`
- `kernel/src/wasm/host/sock.rs`
- `kernel/src/wasm/host/mod.rs`
- `kernel/src/wasm/host/fd.rs`
- `kernel/src/wasm/state.rs`
- `kernel/src/wasm/mod.rs`
- `kernel/src/main.rs`
- `kernel/src/executor/mod.rs`
- `user/server/Cargo.toml`
- `user/server/src/main.rs`
- `user/client/Cargo.toml`
- `user/client/src/main.rs`
- `user/Cargo.toml`
- `user-bin/server.wasm`
- `user-bin/client.wasm`
- `limine.conf`
- `Makefile`
- `CHANGELOG/66-26-05-28-wasix-sockets-demo.md`
