# 417 — DNS Resolver

**Data:** 2026-06-10

## Cosa
Implementata la risoluzione nativa dei nomi a dominio (DNS) appoggiandosi a `smoltcp` e integrando la host function `net_resolve`.

Nello specifico:
- Aggiunta della feature `"socket-dns"` a `smoltcp`.
- `net/mod.rs` ora instanzia un `dns::Socket` e riceve dinamicamente l'array dei `dns_servers` durante il binding DHCP (`Event::Configured`), che usa per istruire il socket DNS tramite `update_servers()`.
- Creato `net/dns.rs` che fa da wrapper async (`start_query` + loop di poll su tick con `Delay::ticks(1)` e `get_query_result`, stesso pattern di `icmp::ping`; timeout di sicurezza 15 s con `cancel_query`).
- Aggiunto `SuspendReason::NetResolve` nella state machine asincrona del kernel (`fiber.rs` e `suspend.rs`).
- Esportata l'API WASM host function `ruos_net_resolve(name_ptr, name_len, addrs_out_ptr, max_addrs, count_out_ptr) -> errno`.
- Creato il tool userland dedicato `resolve` (`resolve google.com`).
- Modificato il tool userland `ping` per invocare automaticamente `ruos_net_resolve` se l'argomento in input non è direttamente parsabile come IPv4 (`parse_ip4`).
- Documentato l'API app-facing in `docs/api/ruos.md`.

## Perché
Il DNS è il requisito essenziale per poter supportare indirizzi simbolici (es. `google.com`) sia nei test ICMP (`ping`) sia in prospettiva di strumenti applicativi e download HTTP/HTTPS futuri (es. `wget`, browser), dove SNI e la validazione del certificato richiedono di possedere l'hostname.

## File toccati
- `kernel/Cargo.toml`
- `kernel/src/net/mod.rs`
- `kernel/src/net/dns.rs`
- `kernel/src/wasm/suspend.rs`
- `kernel/src/wasm/fiber.rs`
- `kernel/src/wasm/host/proc.rs`
- `Makefile`
- `user/resolve/Cargo.toml`
- `user/resolve/src/main.rs`
- `user/ping/src/main.rs`
- `docs/api/ruos.md`
