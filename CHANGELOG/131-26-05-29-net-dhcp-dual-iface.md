# 131 — net: seconda interfaccia Ethernet + DHCPv4

**Data:** 2026-05-29

## Cosa
`NetState` ristrutturato con loopback (127/8, invariato) + interfaccia virtio
Ethernet opzionale (`iface_net`/`dev_net`); socket `dhcpv4` registrato nel
`SocketSet`; `poll()` polla entrambe le interfacce e applica il lease DHCP
(indirizzo IPv4 + default route via `add_default_ipv4_route`), loggando
`net: dhcp bound ip=.. gw=..` al primo bind. Rinominati i campi
`iface → iface_lo` e `device → dev_lo` in `NetState`.

## Perché
Task 7 del Step 14: preparare l'infrastruttura per l'interfaccia reale
virtio-net + acquisizione automatica IP tramite DHCPv4 (necessario per TCP
verso l'esterno e per l'SSH server).

## File toccati
- kernel/src/net/mod.rs
- kernel/src/net/sockets.rs
