# 355 — Netconsole UDP

**Data:** 2026-06-08

## Cosa
4° sink di log: streaming UDP broadcast (255.255.255.255:6666) di ogni riga
kernel INFO+, captabile con `nc -ul 6666`. Gate compile-time
`--features netconsole` (abilita anche `smoltcp/socket-udp`). `emit()` accoda
su un ring dedicato (mai locca NET → no deadlock col net poll task); il drain
+ send vive in `net::poll()`. Al primo bind DHCP viene spinto il backlog del
ring klog (32 KiB) → log da T+0 recuperati anche se la rete sale a ~T+3s.

## Perché
Debug su HW reale con seriale COM rotta. Netconsole dà log passivi/streaming
sulla LAN senza sessione SSH interattiva. Broadcast = zero-config (nessun IP
collector, nessun ARP/gateway). QEMU/VM non raggiungono la LAN fisica →
verifica solo HW reale.

## File toccati
- kernel/src/net/netconsole.rs (nuovo)
- kernel/src/net/mod.rs
- kernel/src/boot/log.rs
- kernel/Cargo.toml
- docs/superpowers/specs/2026-06-08-netconsole-udp-design.md
- docs/superpowers/plans/2026-06-08-netconsole-udp.md
