# Design — Webserver HTTP in ruos (picoserve kernel-native)

**Data:** 2026-06-08
**Stato:** DRAFT / spec di feasibility — da riprendere. Nessun codice scritto.
**Topic:** servizio HTTP nel kernel ruos, riusando il net stack esistente.

---

## 0. Scopo & contesto

Aggiungere a ruos un **servizio webserver HTTP** sullo stesso modello del server
SSH già esistente (`kernel/src/ssh/`, basato su `sunset`). Obiettivo immediato:
servire HTTP/1.1 (static file da VFS + eventuali handler dinamici) su porta TCP.

Questa spec è il risultato di un'analisi di fattibilità. **La decisione
architetturale principale (dove gira il server: kernel-native vs WASM) è ancora
APERTA** — vedi §6. La spec documenta le opzioni, la raccomandazione e il lavoro
di integrazione per la strada raccomandata.

### Contesto codice esistente (verificato 2026-06-08)

- **Net stack pronto**: `smoltcp` integrato (`kernel/src/net/mod.rs`), poll loop
  guidato da `net_poll_task()` nell'executor (ogni ~10 ms, sul **solo core 0 / BSP**).
- **Socket API** (`kernel/src/net/sockets.rs`): pattern listen → accept → recv/send
  già usato da SSH. Funzioni rilevanti:
  - `POOL.alloc_tcp_eth() -> usize` — alloca socket TCP sul SocketSet Ethernet.
  - `POOL.handle(idx) -> Option<SocketHandle>`.
  - `listen(handle, port) -> Result<(), &'static str>` — sincrona.
  - `async accept(handle) -> Result<(), &'static str>` — yield finché Established.
  - `async recv(handle, &mut [u8]) -> Result<usize, &'static str>`.
  - `async send(handle, &[u8]) -> Result<usize, &'static str>`.
  - `try_recv` / `try_send -> Option<usize>` (non-bloccanti), `close(handle)`.
  - Errore = `&'static str` (no enum).
- **SMP reale** (NON single-CPU): executor **per-core** (`kernel/src/executor/mod.rs`,
  `MAX_CPUS=16`). Ruoli: core 0 = `BspIo`, core 1 = `GuiCompositor`, core 2+ =
  `ComputeApp`. Task **pinnati** al core di spawn (no migrazione).
  - `spawn_on(cpu, token) -> Result<(), SpawnError>` — spawn task su core specifico
    (cross-core via `SendSpawner` + IPI `VEC_WAKE`).
  - `pick_compute_core() -> Option<u32>` — round-robin sui core ComputeApp.
  - `smp::pool` — work-stealing per job pure-compute (usato dal compositor a bande).
  - Pattern già rodato `exec_cwasm_parallel` (`kernel/src/wasm/fiber.rs`): la shell
    spawna app `.cwasm` su core compute, lasciando il BSP libero per I/O.
- **Timer** 100 Hz (`kernel/src/timer.rs::ticks() -> u64`, 10 ms/tick).
  `executor::delay::Delay::ticks(n)` è un `Future` che risolve dopo n tick.
  **Non esiste** combinatore `select!`/timeout: si fa con `poll_fn` che corre due
  future (pattern esistente in `kernel/src/pty/mod.rs::slave_read_one_timeout`).
- **Boot wiring**: i servizi si avviano in `kernel/src/boot/phases/userland.rs`
  (es. `ssh::spawn()` + `service::mark_running`). Registry servizi in
  `kernel/src/service/mod.rs::init()`.
- **VFS**: `/mnt` FAT32 montato → static file server gratis. Heap `talc` 128 MiB.

---

## 1. Fattibilità

**ALTA.** Il webserver è lo stesso pattern di SSH (listen → accept → loop). Il
net stack, l'executor async, l'API socket e il wiring di boot esistono tutti. Non
serve nessuna riscrittura del modello.

Crate web `std` (es. **Ferron**, tokio/hyper/mimalloc) sono **inutilizzabili**:
richiedono `std::net`, tokio, OS allocator, syscall — tutto assente in ruos
(no ABI Linux, no ELF, no syscall — vedi CLAUDE.md). Porting = rewrite del 90%.

Crate web **`no_std` async** sono invece compatibili. Candidato:
**`picoserve`** (HTTP server async no_std, axum-like, nato per embassy/smoltcp).

---

## 2. picoserve — compatibilità (verificata)

- Crate sub-dir `picoserve/` nel repo `sammhicks/picoserve`. `#![no_std]`,
  edition 2021, MSRV 1.93 (ruos nightly-2026-05-26 ≫ ok).
- Dipendenze: `embedded-io-async` 0.7 (**required**), `heapless` 0.8 (già in tree
  via sunset), `serde` 1.0 (no_std-capable). Feature opzionali: `embassy`
  (pulla `embassy-net` + `embassy-time`), `tokio`, `json`, `ws`.
- **ruos NON usa `embassy-net`** → la feature `embassy` e `listen_and_serve`
  (accept loop embassy, mono-connessione) sono **inutilizzabili e non servono**.
- API da usare: `serve()` — gestisce **UNA connessione per chiamata** (è il
  mattone, non un limite). Firma (main branch):
  ```rust
  pub async fn serve<S: io::Socket<Runtime>>(self, socket: S)
      -> Result<DisconnectionInfo<...>, Error<S::Error>>
  ```
  Astratto su trait custom: `Runtime` (marker), `io::Socket<Runtime>`
  (`split()` → read/write half, `shutdown()`, `abort()`), `Timer<Runtime>`
  (`run_with_timeout(duration, future)`, `timeout(duration)`).

### ⚠ Decisione versione picoserve (da fissare riprendendo)

Il **main branch** ha rifattorizzato verso l'astrazione `Runtime`/`Socket` (più
superficie da implementare). Versioni **precedenti** usavano `serve(app, config,
&mut buf, socket)` con `socket: embedded_io_async::Read + Write` + un `Timer`
più semplice → **adapter minimo**. **DA FARE riprendendo:** scaricare i trait
esatti (`Runtime`/`io::Socket`/`Timer`) della versione scelta e quantificare
l'adapter al byte. Probabile pin a una versione pre-Runtime per ridurre il lavoro.

---

## 3. Strada raccomandata: A — picoserve kernel-native

Il webserver è un **crate Rust compilato DENTRO il kernel** (come `sunset` per
SSH). Gira in ring 0. **NON è un `.wasm`/`.cwasm`.** Usa `net::sockets` diretto.

### 3.1 Adapter `embedded-io-async` su `net::sockets` (~150–250 LOC)

```rust
struct RuosSocket(SocketHandle);
struct RxHalf(SocketHandle);
struct TxHalf(SocketHandle);

// errore: wrappa &'static str, impl embedded_io_async::Error + ErrorType
struct NetErr(&'static str);

impl embedded_io_async::Read for RxHalf {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, NetErr> {
        net::sockets::recv(self.0, buf).await.map_err(NetErr)   // già async
    }
}
impl embedded_io_async::Write for TxHalf {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, NetErr> {
        net::sockets::send(self.0, buf).await.map_err(NetErr)
    }
    async fn flush(&mut self) -> Result<(), NetErr> { Ok(()) }
}
// io::Socket::split() -> (RxHalf, TxHalf); shutdown/abort -> sockets::close()
```

`net::sockets::recv/send` sono **già `async fn -> Result<usize, &'static str>`**
→ mapping quasi 1:1. Adapter banale.

### 3.2 Timer custom su `Delay` (~40 LOC)

```rust
async fn run_with_timeout<F: Future>(&mut self, d: Duration, fut: F)
    -> Result<F::Output, TimeoutError>
{
    let ticks = (d.as_millis() / 10).max(1) as u64;   // 100 Hz = 10 ms/tick
    // poll_fn che corre `fut` vs Delay::ticks(ticks)
    // (pattern: pty::slave_read_one_timeout)
}
```

### 3.3 Accept loop + concorrenza multi-core

```rust
// task sul BSP (core 0): accetta e distribuisce
#[embassy_executor::task]
async fn http_accept_task() {
    loop {
        let idx = net::sockets::POOL.alloc_tcp_eth();
        let h = net::sockets::POOL.handle(idx).unwrap();
        net::sockets::listen(h, 80).ok();
        if net::sockets::accept(h).await.is_err() { Delay::ticks(50).await; continue; }
        // distribuisci la connessione su un core compute
        match executor::pick_compute_core() {
            Some(core) => { let _ = executor::spawn_on(core, http_conn_task(h)); }
            None        => { http_conn_task(h).await; } // fallback: servi qui
        }
    }
}

#[embassy_executor::task(pool_size = 16)]   // N handler concorrenti
async fn http_conn_task(h: SocketHandle) {
    let app = /* picoserve::Router::new()... */;
    let _ = picoserve::serve(app, RuosSocket(h)).await;
    net::sockets::close(h);
}
```

- **Concorrenza vera multi-core**: conn 1 → core 2, conn 2 → core 3, ... via
  `spawn_on(pick_compute_core(), ...)`. Ogni core polla la sua coda
  indipendentemente → handler in parallelo reale (CPU-bound: parsing, routing,
  build response, TLS, eventuale exec wasm).

### 3.4 Wiring boot

- `kernel/src/boot/phases/userland.rs`: dopo `ssh::spawn()`, avviare il servizio
  HTTP (spawn `http_accept_task` o pattern equivalente) + `service::mark_running`.
- `kernel/src/service/mod.rs::init()`: `register("http", BUILTIN_PATH, true)`.

---

## 4. Il caveat I/O (importante, da tenere a mente)

Il parallelismo è **vero per la CPU**, ma il net stack ha un collo di bottiglia
sul **core 0**:

1. `net_poll_task` (poll smoltcp) gira **solo sul BSP**.
2. `net::sockets::recv/send` lockano un **`Mutex` globale `NET`** — ogni byte
   in/out passa per quel lock.

| Lavoro | Scala su N core? |
|---|---|
| Parsing, routing, logica app, build response, TLS, exec wasm | ✅ parallelo vero |
| Shuffling byte socket (recv/send) + poll smoltcp | ❌ serializzato su core 0 + lock NET |

**Per decine di client = nessun problema** (il tempo sta nella logica, non nei
byte). Bottleneck solo a throughput estremo (saturazione NIC, migliaia req/s).
Eventuale lavoro futuro: poll multi-core / lock più fine — **fuori scope ora**.

---

## 5. Limiti di scala (da dimensionare)

Non è picoserve a limitare i client concorrenti, ma 3 risorse:

| Risorsa | Dove | Tuning |
|---|---|---|
| N socket Ethernet | `SockPool` / smoltcp `net_sockets` SocketSet | alzare gli slot |
| N task | `pool_size = N` su `http_conn_task` + `task-arena-size` embassy | Cargo feature |
| RAM/conn | rx+tx buffer picoserve + buffer smoltcp per socket | heap talc 128 MiB |

**DA FARE riprendendo:** misurare quanti slot eth ha oggi `SockPool` e la taglia
del SocketSet smoltcp → tetto reale di client concorrenti senza modifiche.

---

## 6. ⚠ DECISIONE APERTA — dove gira il server (kernel-native vs WASM)

Tre architetture distinte. La spec raccomanda **A** ma la scelta è del repo owner.

| | Cos'è | WASM? | Velocità | Socket oggi | Effort |
|---|---|---|---|---|---|
| **A** | picoserve kernel-native (come SSH) | ❌ | nativa | ✅ `net::sockets` diretto | basso |
| **B** | webserver come app `.wasm` (wasmi) | ✅ interprete | lenta | ✅ `sock_accept`+`fd_read/write` | medio |
| **C** | webserver come `.cwasm` (Wasmtime AOT) | ✅ AOT | quasi-nativa | ❌ **socket non esposti** | alto |

- **A** — semplice, veloce ORA. Ring 0, non sandboxato. **Raccomandato per il primo
  servizio funzionante.**
- **B** — socket WASI già presenti (wasmi), ma interprete lento + no `poll_oneoff`
  su socket (1 client/fiber). OK demo, no produzione.
- **C** — coerente col north-star ("tutto userspace = WASM") ma Wasmtime AOT **non
  espone host-fn socket** (solo gfx + WASI-fs). Servirebbe **estendere il layer
  WASI socket di Wasmtime** (`wt/wasi.rs` o nuovo `wt/sock.rs`): accept/recv/send.
  Complicazione: Wasmtime AOT è **sincrono** (no yield cooperativo dei fiber wasmi)
  → call socket bloccanti via `vfs::block_on` = blocca un core. Da progettare a
  parte.

### Ambiguità "app" da chiarire col owner

1. **Il webserver stesso** è cwasm? → richiede C (estendere WASI socket). Effort alto.
2. **Il webserver (A, nativo) serve/esegue app cwasm per richiesta** (stile CGI:
   `GET /foo` → `exec_cwasm_parallel("foo.cwasm")` su core compute → response)?
   → facile e potente: server nativo + handler che riusa il pattern cwasm
   esistente. **Le app dinamiche sono cwasm, il server no.** Probabilmente è
   questo il modello desiderato.

**→ Da decidere riprendendo. Cambia tutto il design.**

---

## 7. Piano a fasi (bozza, per quando si riprende)

1. **Fissare decisione §6** (A consigliato) + pin versione picoserve (§2).
2. **PoC minimo**: 1 connessione, single core. Adapter `embedded-io-async`
   (RxHalf/TxHalf/NetErr) + Timer custom. `serve()` che risponde
   `200 OK "hello"` a `GET /`. Validare l'adapter in QEMU (`make run-test`).
3. **Static file server**: router che serve da `/mnt` (VFS/FAT32).
4. **Concorrenza multi-core**: accept loop + `spawn_on(pick_compute_core())`,
   `pool_size` adeguato. Dimensionare SockPool (§5).
5. **Wiring servizio**: `service::register("http", ...)` + spawn in
   `boot/phases/userland.rs` + `mark_running`.
6. **(Opzionale) handler dinamici cwasm** (modello §6.2): `exec_cwasm_parallel`
   per-richiesta.
7. **(Futuro) TLS** — valutare crate no_std (es. `embedded-tls`). Fuori scope MVP.

Ogni fase: spec → piano → impl, con `make iso` + `make run-test`, e changelog per
ogni modifica (regola CLAUDE.md).

---

## 8. Rischi / da verificare

- Trait `Runtime`/`io::Socket`/`Timer` esatti della versione picoserve scelta (§2).
- `serde` no_std entra pulito nel build kernel (`default-features=false`, `derive`).
- Allineamento versioni `heapless` 0.8 / `serde` col resto del tree (sunset).
- Taglia SockPool / SocketSet smoltcp = tetto client (§5).
- Lock `NET` globale come bottleneck I/O a throughput alto (§4) — accettato per MVP.

---

## 9. Riferimenti codice

| Cosa | File |
|---|---|
| Net init + poll | `kernel/src/net/mod.rs` |
| Socket API (listen/accept/recv/send) | `kernel/src/net/sockets.rs` |
| SSH server (template) | `kernel/src/ssh/server.rs`, `kernel/src/ssh/sunset_io.rs` |
| Executor per-core + spawn_on + pick_compute_core | `kernel/src/executor/mod.rs` |
| Compute pool work-stealing | `kernel/src/smp/pool.rs` |
| Pattern cwasm su core compute | `kernel/src/wasm/fiber.rs::exec_cwasm_parallel` |
| Timer / Delay | `kernel/src/timer.rs`, `kernel/src/executor/delay.rs` |
| Timeout via poll_fn (pattern) | `kernel/src/pty/mod.rs::slave_read_one_timeout` |
| Boot wiring servizi | `kernel/src/boot/phases/userland.rs` |
| Registry servizi | `kernel/src/service/mod.rs` |
| WASI socket wasmi (strada B) | `kernel/src/wasm/host/sock.rs`, `host/fd.rs` |
| WASI Wasmtime AOT (strada C, no socket) | `kernel/src/wasm/wt/wasi.rs` |
