# 456 — net.dial: porta locale effimera rotante (fix chiusure TCP intermittenti)

**Data:** 2026-06-11

## Cosa

`net.dial` (window host module, `wt/net.rs`) sceglieva la porta locale in modo
deterministico per slot: `local_port = 49152 + (idx & 0x3FFF)`. Il pool socket è
globale e i socket chiusi restano nel SocketSet finché non raggiungono `Closed`
(reclaim al `alloc_tcp*` successivo, vedi 452). Quando uno slot veniva
riassegnato, la NUOVA connessione riusava la STESSA porta locale; se il vecchio
socket verso lo stesso remote era ancora lingering (TIME_WAIT / in chiusura) la
4-tupla collideva in smoltcp → RST / chiusura prematura post-handshake /
trasferimento troncato, **intermittente**.

Ora un contatore monotono globale (`NEXT_EPHEMERAL: AtomicU32`) cammina su tutte
le 16384 porte effimere `49152..=65535` prima di riusarne una — molto più a
lungo di qualunque socket lingering. `fetch_add` Relaxed, nessun lock.

## Perché

Sintomo lato app (viewer HTTPS): `example.com` sempre OK, ma `motherfuckingwebsite.com`,
`rust-lang.org`, ecc. a volte chiudevano a `raw=0` subito dopo l'handshake, e
trasferimenti grossi (`lite.cnn.com`) si troncavano a metà (es. 294912/340275 B).
La diagnostica nel viewer ha provato che la richiesta HTTP era spedita
correttamente (handshake completo, `wants_write=false`, niente in coda TX): la
connessione moriva sotto, non per colpa dell'app. Causa = riuso porta locale.

## File toccati

- kernel/src/wasm/wt/net.rs
- (doc app) ruos-test/api/net.md — nota `dial` aggiornata
