# 132 — net: SocketSet dedicata per DHCP

**Data:** 2026-05-29

## Cosa
Il socket dhcpv4 ora vive in una SocketSet separata (net_sockets) pollata SOLO
da iface_net. iface_lo (loopback, medium Ip) non lo tocca più.

## Perché
smoltcp panica (dhcpv4.rs:557) se un socket dhcpv4 è pollato da un'interfaccia
non-Ethernet. Con la SocketSet condivisa, iface_lo lo colpiva a ogni poll →
KERNEL PANIC. Trovato a runtime nel gate Task 8.

## File toccati
- kernel/src/net/mod.rs
