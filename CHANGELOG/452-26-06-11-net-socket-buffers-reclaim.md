# 452 — Socket TCP: buffer 64K/8K, riuso slot su close, DNS pool 8

**Data:** 2026-06-11

## Cosa

Tre fix kernel-side per il fetch HTTP di pagine reali (punti B, C, E
dell'analisi browser):

- **B — buffer socket**: `BUF_SIZE=4096` (unico per RX/TX) →
  `RX_BUF_SIZE=64 KiB` + `TX_BUF_SIZE=8 KiB`. La finestra TCP advertita passa
  da 4K a 64K (throughput reale invece di mille poll-round per pagina). Il cap
  `.min(4096)` in `net.read` ora è legato a `RX_BUF_SIZE`.
- **C — riuso slot pool**: prima `net.close` chiudeva il socket ma lo slot
  pool e il socket smoltcp restavano allocati per sempre (SocketSet cresceva
  monotonicamente). Ora `net.close` → `POOL.release(idx)`: close graceful +
  flag `closing`; al successivo `alloc_tcp*` un passo `reclaim()` rimuove dal
  SocketSet i socket arrivati a `Closed` (FIN handshake completo) e libera lo
  slot. Dopo `close` l'handle guest è invalido (può essere riassegnato).
  Il path wasmi (`fd_close` restore-arm) resta invariato di proposito: lì il
  socket loopback fd4 deve sopravvivere al close.
- **E — DNS pool**: `dns_task pool_size` 4 → 8 (pagine multi-hostname).

Nota: `reclaim()` non tiene mai pool-lock e `NET` insieme (3 fasi:
collect → remove → free) per evitare inversione di lock con `alloc_tcp_in`.

## Perché

Una pagina web reale = decine di sotto-risorse → decine di connessioni TCP per
load. Con slot mai liberati il SocketSet si riempiva di handle morti e ogni
socket da 2×4K strozzava il transfer. Prerequisito kernel per il viewer
HTTP/HTTPS (TLS resta app-side, opzione A1 — zero lavoro kernel).

## File toccati

- kernel/src/net/sockets.rs
- kernel/src/wasm/wt/net.rs
- docs/api/net.md
