# Networking + SSH

> **Stato:** bozza
> **Aggiornato:** 2026-06-10
> **Fonti:** `kernel/src/net/`, `kernel/src/ssh/`
> **Spec collegate:** `docs/superpowers/specs/2026-05-29-rust-virtio-net-dhcp-csprng-design.md`,
> `docs/superpowers/specs/2026-05-29-rust-nic-drivers-design-and-plan.md`

## Cos'è

Il networking di ruOS è uno **stack TCP/IP completo** (`smoltcp`) con tre driver
NIC e un **server SSH** integrato nel kernel. La rete non è emulata: ruOS
gestisce veri pacchetti Ethernet, fa DHCP, risponde a ICMP, apre connessioni TCP
e ha un server SSH con cui ci si collega da un altro computer.

## Dove vive

| File / cartella | Ruolo |
|-----------------|-------|
| `kernel/src/net/mod.rs` | Stack `smoltcp`, interface, socket set, poll loop |
| `kernel/src/net/virtio.rs` | Driver **virtio-net** (paravirtuale QEMU) |
| `kernel/src/net/e1000.rs` | Driver **Intel e1000** |
| `kernel/src/net/rtl8169.rs` | Driver **Realtek RTL8169** |
| `kernel/src/net/device.rs` | Trait `NicDriver`, dispatch al driver attivo |
| `kernel/src/ssh/` | Server SSH: `sunset` bridge, hostkey, auth, PTY spawn |
| `kernel/src/rng.rs` | ChaCha20 CSPRNG (seed RDRAND) — necessario per crypto |

## Modello

```
              smoltcp TCP/IP stack
         ┌──────────┬────────────┐
         │ virtio   │ e1000      │ rtl8169
         │ (QEMU)   │ (QEMU/HW) │ (HW)
         └──────────┴────────────┘
                     │
              ┌──────▼──────┐
              │  DHCP client │  (auto-config at boot)
              │  ICMP echo   │  (ping host fn)
              │  TCP sockets │  (nc, wget, SSH)
              └──────┬──────┘
                     │
              ┌──────▼──────┐
              │  SSH server  │  sunset + ed25519
              │  (port 22)   │  password + pubkey
              └─────────────┘
```

### Driver NIC

Tre backend, selezionati al boot in base al dispositivo PCI trovato:

| Driver | PCI VID:DID | Contesto |
|--------|-------------|----------|
| **virtio-net** | 1AF4:1000 | QEMU paravirtuale (il più comune in dev) |
| **e1000** | 8086:100E | QEMU `-netdev e1000`, VirtualBox |
| **rtl8169** | 10EC:8168/8136 | Hardware reale (Realtek GbE) |

Ciascun driver implementa il trait `NicDriver`: init, TX (send packet), RX (poll
received packets). I buffer DMA sono allocati dal frame allocator con frame
contigue.

### Stack TCP/IP

`smoltcp` gestisce:
- **DHCP**: auto-configurazione IP/netmask/gateway al boot. La host fn
  `net_dhcp_renew` permette di riavviare il client.
- **ICMP**: echo request/reply per la host fn `ping()`.
- **TCP**: connessioni in uscita (`tcp_dial`) e in ascolto (`sock_accept`).
  I socket TCP sono mappati su fd WASI — il guest usa `fd_read`/`fd_write`.
- **Interfaccia statica**: `net_set_static` configura IP/prefix/gateway senza DHCP.

Il poll loop di rete gira come task async sull'executor del BSP — raccoglie
pacchetti RX dal driver, li passa a smoltcp, e invia i TX pending.

## SSH

Il server SSH usa la libreria **`sunset`** (vendored in `third_party/sunset/`):

- **Hostkey**: ed25519 (generato all'avvio; persistito su `/mnt` se disponibile,
  altrimenti effimero in RAM).
- **Auth password**: PBKDF2-HMAC-SHA256 contro `/mnt/passwd` (se esiste), oppure
  il default compilato (`RUOS_DEFAULT_PASSWORD`, default: `ruos`).
- **Auth pubkey**: ed25519, chiave in `/mnt/auth.key` (formato OpenSSH).
- **Shell interattiva**: spawna `/bin/shell.wasm` su un PTY dedicato; il canale SSH
  legge/scrive dal PTY master.
- **Exec non-interattivo**: esegue un comando e restituisce l'output.

Il server gira **nel kernel** (ring 0); l'app che spawna (la shell WASM) è la
parte sandboxata. Il release build è necessario — la crypto in debug è troppo
lenta (KEX > 60 s).

### Wi-Fi (RTL8188EU)

Un driver USB Wi-Fi per il chipset **RTL8188EU** (dongle USB 2.4 GHz) è in
sviluppo:

- **`wifi_scan`**: porta il chip online (power-on + firmware 8051 + MAC/BB/RF
  init), scansione passiva 2.4 GHz, ritorna SSID/canale/security.
- **`wifi_connect`**: open-system auth + WPA2 association + 4-way handshake
  (HMAC-SHA1 PTK/MIC, AES GTK unwrap, key install nel HW CAM).
- Il path dati cifrato (CCMP TX/RX) e DHCP over Wi-Fi sono lavori in corso.

## Contratti

- La rete si avvia alla **fase 10** del boot (userland), dopo PCI e VFS.
- I socket TCP sono trasparenti per il guest: `tcp_dial` ritorna un fd WASI che
  il guest legge/scrive con le normali `fd_read`/`fd_write`. La sospensione
  (attesa di dati) passa per la fiber.
- Il server SSH è un servizio del kernel, non un processo WASM. È registrato nel
  service manager e può essere listato/startato via `service`.

## Vincoli e limiti

- **Una sessione SSH alla volta**: il server accetta una sola connessione attiva.
- **No DNS**: le host fn di rete (`tcp_dial`, `ping`) accettano solo IP literal
  (dotted-quad IPv4). Il tool `wget` è HTTP/1.0, no HTTPS.
- **No IPv6**: tutto lo stack è solo IPv4.
- **Port 22 fisso**: non configurabile.
- **No forwarding/SFTP**: solo shell interattiva ed exec.
- **Release build obbligatorio**: il key exchange in debug profile è troppo lento
  per il timeout SSH standard.

## Insidie / note

- Il CSPRNG (`rng.rs`) è critico: la generazione della hostkey SSH e tutto il TLS
  dipendono da esso. È seedato da `RDRAND` — senza `RDRAND` (CPU molto vecchie)
  la crypto non funziona.
- Il password di default (`ruos`) è in chiaro nel binario del kernel: non è un
  meccanismo di sicurezza, ma una comodità per demo.
- La host fn `net_dhcp_renew` non aspetta il completamento del DHCP: ritorna
  subito e il DHCP avviene in background nel poll loop.

## Vedi anche

- [Boot a fasi](boot-phases.md) — fase 10 (networking)
- [Runtime WASM](wasm-runtime.md) — come i socket diventano fd
- [Architettura — panoramica](../architecture/overview.md)
- [Indice della wiki](../README.md)
