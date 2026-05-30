# 149 — nc + date + ruos::tcp_dial + RTC

**Data:** 2026-05-29

## Cosa

### Kernel

- `kernel/src/rtc.rs` — CMOS RTC reader (port 0x70/0x71), double-read
  + UIP wait, BCD/binary + 12/24h transparent, `to_unix_epoch`
- `ruos::time_get(year_ptr..sec_ptr, epoch_ptr) -> errno` — espone RTC
- `ruos::tcp_dial(ip[4], port, fd_out_ptr) -> errno` — alloc TCP
  socket dal pool, inject FdEntry::Socket nel caller, scrivi FD a
  `fd_out_ptr`, trap `SuspendReason::SockConnect` (fiber attende
  Established prima di tornare al wasm)
- `smoltcp` feature `socket-icmp` aggiunta (per future ping)

### User tools

- **nc** `<ip> <port>` — TCP client raw. Apre socket via tcp_dial,
  stdin ↔ socket bidirezionale, ^D chiude
- **date** [+%s] [-u] — legge RTC, formato `YYYY-MM-DD HH:MM:SS UTC`
  o unix epoch con `+%s`

## Test

`make run-test` → TEST_PASS.

Manuale:
```
ruos:/$ date
2026-05-29 19:34:56 UTC
ruos:/$ date +%s
1780526096
ruos:/$ nc 10.0.2.2 80
GET / HTTP/1.0

HTTP/1.0 200 ...
```

## Note tecniche

`tcp_dial` non rolla back FD se connect fallisce (leaked entry); a
posteriori, fd_close è OK perché socket pool entry rimane finché
non rimpiazzata. Future: SockConnect dispatch ritorna errno e cleanup.

## File toccati

- kernel/src/rtc.rs (nuovo)
- kernel/src/main.rs (`mod rtc;`)
- kernel/src/wasm/host/proc.rs (`ruos_time_get`, `ruos_tcp_dial`)
- kernel/Cargo.toml (smoltcp socket-icmp)
- user/{nc,date}/Cargo.toml + src/main.rs (nuovi)
- user/Cargo.toml (members)
- Makefile (BIN_TOOLS)
- limine.conf (modules)
- CHANGELOG/149-26-05-29-nc-date-tcp-dial-rtc.md (questo)
