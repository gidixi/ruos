# 423 — fix: kernel panic nel socket DNS con >1 server DHCP

**Data:** 2026-06-10

## Cosa
`net/mod.rs`: alla DHCP-bind, capo la lista dei DNS server a
`DNS_MAX_SERVER_COUNT` (1) prima di `dns_socket.update_servers(...)`.

## Perché
Il socket DNS di smoltcp 0.11 tiene i server in un `heapless::Vec` di capacità
`DNS_MAX_SERVER_COUNT` (default **1**). `update_servers` fa `Vec::from_slice(...)
.unwrap()` → **PANIC** se gli passi più server della capacità
(`smoltcp/src/socket/dns.rs:170`). Il codice passava TUTTI i DNS del lease DHCP.

Sintomo: su **VBox** (NAT offre 2 DNS) il sistema bootava fino alla shell poi
**KERNEL PANIC → reboot loop**. Su QEMU (NAT dà 1 DNS, 10.0.2.3) non si vedeva.
Regressione introdotta con la feature DNS resolver (commit eb38b13). Nessun
legame con USB-WiFi (riproduceva senza chiavetta).

## Verifica
VBox `ruos` (6 vCPU, EFI): boot → shell stabile, nessun panic (prima:
`KERNEL PANIC ...dns.rs:170 called Result::unwrap() on an Err value`).

## File toccati
- kernel/src/net/mod.rs
