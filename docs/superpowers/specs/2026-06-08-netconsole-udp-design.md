# Netconsole UDP — design

**Data:** 2026-06-08
**Stato:** approvato, pre-implementazione
**Branch:** `feat/usb-msc-livecd` (per richiesta utente: non creare branch nuovo)

## Contesto / motivazione

Debug su hardware reale con seriale COM rotta. Il driver RTL8169 (vedi
`2026-06-08-rtl8169-driver-design.md`) ha portato su la rete; SSH inbound dà già
una shell remota. Netconsole aggiunge **streaming passivo dei log kernel via UDP
broadcast**: ogni riga `binfo!`/`bwarn!`/`berr!` viene spedita sulla LAN, captabile
con `nc -ul 6666` da qualsiasi PC, senza sessione interattiva e senza che ruos
conosca l'IP del collector.

Il logger (`kernel/src/boot/log.rs::emit`) ha già 3 sink: ring `dmesg` (`klog`),
seriale COM1, framebuffer. Netconsole è un **4° sink**.

## Scope

**In scope:**
- Sink UDP broadcast (`255.255.255.255:6666`) per ogni riga di log INFO+.
- Flush del backlog `dmesg` (ring klog) al primo bind DHCP → log da T+0 recuperati
  anche se la rete sale a ~T+3s.
- Gate **compile-time** `--features netconsole`: zero codice/overhead quando off.

**Out of scope (drop espliciti):**
- Destinazione unicast / collector configurabile (solo broadcast).
- Toggle a runtime da shell (solo feature compile-time).
- Ricezione comandi via UDP (è solo uscita log, non una console bidirezionale).
- Filtri per livello/modulo (si manda tutto INFO+, come la seriale).
- QEMU/VM: il broadcast slirp non raggiunge la LAN fisica → verifica solo HW reale.

## Vincolo critico: reentrancy / deadlock

`emit()` gira anche **dentro il net poll task**: il driver NIC chiama
`bwarn!("net", "tx queue full")` mentre tiene il lock `NET` + `iface.poll`. Se il
sink netconsole inviasse UDP **sincrono** da `emit()`, ri-locerebbe `NET` →
**deadlock**.

→ Architettura obbligata: `emit()` **solo accoda** su un ring dedicato (locca solo
quel ring, mai `NET`). Il drain+send vive in `net::poll()`, che già tiene `NET`.

Inoltre il path di drain **non deve mai loggare** (`binfo!`/`bwarn!`) — un errore
di send che logga rientrerebbe in `enqueue` → feedback loop. Errori silenziati.

## Architettura

### Nuovo file `kernel/src/net/netconsole.rs` (tutto `#[cfg(feature = "netconsole")]`)

**Stato statico:**
- `NC_RING: spin::Mutex<NcRing>` — ring byte bounded da 48 KiB (basta a contenere il
  backlog klog 32 KiB + burst). Produttore=`enqueue`, consumatore=`drain`. Cursori
  head/tail consumabili (klog::read NON avanza il cursore → serve coda vera).
  Overflow → scarta i byte più vecchi.
- `NC_HANDLE: spin::Mutex<Option<SocketHandle>>` — handle del socket UDP in
  `net_sockets`.
- `BOUND: AtomicBool` — true dopo il primo bind DHCP.
- `JUST_BOUND: AtomicBool` — one-shot per innescare il flush del backlog.

**API:**
- `enqueue(bytes: &[u8])` — se `!BOUND` ritorna subito (no-op: i log pre-bind stanno
  in klog e verranno recuperati dal backlog flush → niente overflow del ring,
  niente duplicati). Se `BOUND`, push su `NC_RING`. Chiamato da `emit()`.
- `init(net_sockets: &mut SocketSet)` — crea `udp::Socket` (rx buf piccolo, tx buf
  ~8 KiB / 32 pkt-meta), `bind(6666)`, salva l'handle. Chiamato da `net::init`.
- `mark_bound()` — setta `BOUND=true` e `JUST_BOUND=true`. Chiamato nel blocco DHCP
  di `net::poll` alla transizione `dhcp_bound` false→true.
- `on_poll(net_sockets: &mut SocketSet)` — chiamato in cima a `net::poll()` PRIMA
  degli `iface.poll` ethernet (così la stessa poll trasmette):
  1. se `JUST_BOUND` (swap→false): legge l'intero ring klog (`klog::read`) in un buf
     temporaneo e lo `enqueue`a (backlog).
  2. drena `NC_RING` in chunk ≤512 B (spezzando sull'ultimo `\n` ≤512 per leggibilità,
     altrimenti taglio netto a 512), fino a max 8 datagram/tick, e per ciascuno
     `udp.send_slice(chunk, (Ipv4Address::BROADCAST, 6666))`. Send fallito (buffer
     pieno) → rimette indietro/lascia nel ring e smette per questo tick. Mai logga.

### Hook nei file esistenti

- **`kernel/src/boot/log.rs::emit`** — dopo `klog::push(bytes)`:
  ```rust
  #[cfg(feature = "netconsole")]
  crate::net::netconsole::enqueue(bytes);
  ```
- **`kernel/src/net/mod.rs`**:
  - `pub mod netconsole;` (cfg-gated).
  - in `init()`, dopo aver creato `net_sockets` e prima di costruire `NetState`:
    `#[cfg(feature="netconsole")] netconsole::init(&mut net_sockets);`
  - in `poll()`, subito dopo `let t = now();` e il poll loopback, PRIMA del poll
    delle iface ethernet:
    `#[cfg(feature="netconsole")] netconsole::on_poll(&mut net.net_sockets);`
  - nel blocco DHCP (transizione a `dhcp_bound = true`, ~mod.rs:140):
    `#[cfg(feature="netconsole")] netconsole::mark_bound();`
- **`kernel/Cargo.toml`** — aggiungere `netconsole = []` alla sezione `[features]`.

### Perché broadcast semplifica

Dest MAC `ff:ff:ff:ff:ff:ff` → nessun ARP, nessun gateway necessario → trasmette
appena l'iface ha un IP (post-bind), senza dipendere dalla risoluzione neighbor.
Il TX broadcast su RTL8169 è già provato (DHCP).

## Data flow

`binfo!` → `emit()` → `klog::push` (+ `netconsole::enqueue` se BOUND) → (next tick)
`net::poll` → `on_poll`: [flush backlog se appena bound] + drena ring → `udp.send_slice`
broadcast → `iface.poll` ethernet TX → datagram sulla LAN → `nc -ul 6666`.

## Error handling

- Pre-bind: `enqueue` no-op; tutto in klog; recuperato al flush.
- UDP tx buffer pieno: drain si ferma per il tick, riprende al prossimo. Nessun log
  perso dal ring (resta in coda finché c'è spazio; se il ring satura → drop vecchi).
- Rete giù / socket assente: `on_poll` no-op.
- Nessun `binfo!`/`bwarn!`/`berr!` nel path drain/send (anti-loop).

## Testing / verifica

QEMU/VM broadcast non raggiunge la LAN fisica → **verifica solo HW reale**.
Nessun test automatico.

Criteri di accettazione:
1. `make iso` (feature **off**) builda identico a prima; nessun simbolo netconsole;
   `make run-test` invariato.
2. `make iso CARGO_FEATURES=netconsole` builda.
3. Su HW reale con `--features netconsole`: avvia `nc -ul 6666` (o
   `socat -u UDP-RECV:6666 -`) su un altro PC della LAN; dopo il bind DHCP di ruos
   compare lo **stream live** dei log + il **backlog** da `[T+0.0..]`.

Build via WSL:
`wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/... && make iso CARGO_FEATURES=netconsole'`.

## File toccati

- `kernel/src/net/netconsole.rs` (nuovo)
- `kernel/src/net/mod.rs` (mod + init + poll hook + bind flag)
- `kernel/src/boot/log.rs` (enqueue hook)
- `kernel/Cargo.toml` (feature `netconsole`)
- `CHANGELOG/355-26-06-08-netconsole-udp.md` (nuovo)
