# 148 — ifconfig + static IP / DHCP renew host fns

**Data:** 2026-05-29

## Cosa

### Kernel host fn (`ruos` module)

`ruos::net_set_static(ip0..3, prefix, gw0..3, gw_present) -> errno`:
- Iface attiva: `iface_net` xor `iface_nic`
- `iface.update_ip_addrs` rimuovi + push nuovo IpCidr/v4
- `routes_mut.remove_default_ipv4_route` + `add_default_ipv4_route(gw)`
- Cancella DHCP socket (operatore override DHCP)
- Log `net: static ip=... gw=...`

`ruos::net_dhcp_renew() -> errno`:
- Se DHCP socket assente, ri-aggiungi → next poll → DISCOVER/OFFER cycle

### Userland tool `ifconfig`

```
ifconfig                                   # show all
ifconfig eth0 192.168.1.10/24              # static IP only
ifconfig eth0 192.168.1.10/24 gw 192.168.1.1   # IP + gateway
ifconfig eth0 dhcp                          # restart DHCP
```

Iface name attualmente informational (single Ethernet iface attiva).
Future: multi-NIC dispatch.

## Test

`make run-test` → TEST_PASS.

Test interattivo:
```
ruos:/$ ifconfig
lo    127.0.0.1/8
eth0  10.0.2.15/24 mac=52:54:00:12:34:56 gw=10.0.2.2
ruos:/$ ifconfig eth0 192.168.50.10/24 gw 192.168.50.1
ifconfig: applied static IP
ruos:/$ ifconfig
lo    127.0.0.1/8
eth0  192.168.50.10/24 mac=52:54:00:12:34:56 gw=192.168.50.1
ruos:/$ ifconfig eth0 dhcp
ifconfig: DHCP restart requested
```

## File toccati

- kernel/src/wasm/host/proc.rs (host fn net_set_static + net_dhcp_renew)
- user/ifconfig/Cargo.toml + src/main.rs (nuovo)
- user/Cargo.toml (member)
- Makefile (BIN_TOOLS)
- limine.conf (/bin/ifconfig.wasm)
- CHANGELOG/148-26-05-29-ifconfig-static-ip.md (questo)
