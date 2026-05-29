# 123 — lspci + ip wasm tools

**Data:** 2026-05-29

## Cosa

Due nuovi user tool `.wasm`:

- **`lspci`** — lista i device PCI enumerati a boot (Step 13).
- **`ip`** — mostra interfacce di rete (lo + eth0 con MAC, IP/CIDR, gw).

Output esempio in QEMU q35 + virtio-net:
```
00:00.0  8086:29c0  06 00 00  Host bridge
00:02.0  1b36:000d  0c 03 30  xHCI USB controller
00:03.0  1af4:1000  02 00 00  Ethernet controller
00:1f.2  8086:2922  01 06 01  SATA controller
lo    127.0.0.1/8
eth0  10.0.2.15/24 mac=52:54:00:12:34:56 gw=10.0.2.2
```

## Come

2 nuove host fn nel modulo `ruos` (`kernel/src/wasm/host/proc.rs`):

- `ruos::pci_list(buf_ptr, buf_len, used_ptr) -> errno`
- `ruos::net_iface(buf_ptr, buf_len, used_ptr) -> errno`

Entrambe scrivono testo già formattato (kernel-side render). Pattern
ENOBUFS (errno 8): se `buf_len < required`, ritorna 8 e scrive
`required` in `used_ptr`; il tool ri-alloca e riprova. Semplifica i
crate userland (no parsing binary).

`pci_class_name(class, sub, prog_if) -> &'static str` mappa coppie note
(Mass storage, Network, Bridge, USB xHCI/EHCI, ecc.) per output
human-readable.

`net_iface` legge `crate::net::NET` sotto Mutex, itera `iface_lo` +
`iface_net` (se presente). Gateway estratto via `routes_mut().update`
(unico path read-only di smoltcp Routes).

User crate `user/lspci`, `user/ip` — 30 righe ciascuno: chiamano host
fn, gestiscono ENOBUFS, `print!` UTF-8.

## File toccati

- kernel/src/wasm/host/proc.rs (host fn + class mapping)
- user/Cargo.toml (members += lspci, ip)
- user/lspci/{Cargo.toml, src/main.rs} (nuovo)
- user/ip/{Cargo.toml, src/main.rs} (nuovo)
- Makefile (BIN_TOOLS += lspci ip)
- limine.conf (module_path += /bin/lspci.wasm /bin/ip.wasm)
- user-bin/init.sh (smoke: lspci + ip in fondo)
- CHANGELOG/123-26-05-29-lspci-ip-tools.md (questo)

## Test

`make run-test` → TEST_PASS (shell + PCI + xHCI + DHCP asserts).
Init.sh ora include `lspci` e `ip` come ultimi cmd: output visibile
nel serial log conferma il rendering.
