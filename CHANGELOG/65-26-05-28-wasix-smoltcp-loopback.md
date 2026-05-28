# 65 — smoltcp + Loopback device + net_poll_task (Step 10 Task 5)

**Data:** 2026-05-28

## Cosa

- Aggiunta dep `smoltcp = "0.11"` `default-features=false` con
  `alloc + medium-ip + proto-ipv4 + socket-tcp`.
- `kernel/src/net/mod.rs` (nuovo): `NetState` globale (Interface +
  Loopback + SocketSet) in `Mutex<Option<_>>`. `init()` lo popola
  con IP 127.0.0.1/8. `poll()` chiamata periodica.
- `kernel/src/net/loopback.rs` (nuovo): wrapper triviale su
  `smoltcp::phy::Loopback`.
- `kernel/src/net/sockets.rs` (nuovo, vuoto): placeholder per Task 6.
- `kmain` chiama `net::init()` dopo `modules::mount_all()`.
- `executor` spawna `net_poll_task` accanto agli altri task. 10ms via
  `Delay::ticks(1)`.
- HELLO → `ruos: net init ok addr=127.0.0.1/8`.

## Perché

Quinto task dello Step 10. Stack di rete embedded sopra Loopback.
Sblocca Task 6 (sock_* host fns su 127.0.0.1). Niente NIC reale
fino a Step 14.

## File toccati

- kernel/Cargo.toml
- kernel/src/net/mod.rs (nuovo)
- kernel/src/net/loopback.rs (nuovo)
- kernel/src/net/sockets.rs (nuovo)
- kernel/src/main.rs
- kernel/src/executor/mod.rs
- Makefile
- CHANGELOG/65-26-05-28-wasix-smoltcp-loopback.md (nuovo)
