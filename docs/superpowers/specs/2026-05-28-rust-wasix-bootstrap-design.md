# Step 10 — WASIX bootstrap (wasmi + host fns + smoltcp loopback)

**Data:** 2026-05-28
**Roadmap step:** 10 (pivot — vedi nota sotto)
**Stato:** spec approvata, da implementare

## Pivot dalla roadmap originale

La roadmap text di `CLAUDE.md` step 10 dice "WASI Preview 1". Pivot deciso
in brainstorm 2026-05-28: passiamo a **WASIX** (Wasmer Inc estensione di
WASI P1). Motivazione: ecosystem pre-built (bash, python, vim, curl)
sblocca userland reale a Step 13 senza scrivere shell custom. Step 10
diventa "WASIX bootstrap" monolitico — runtime + host fns + loopback
sockets — perché lo scope minimale dell'utente includeva esplicitamente
"sockets + fd + clock + path".

I bullets non più validi della roadmap originale (cf. CLAUDE.md):

- ❌ "wasmi preferito, altrimenti WAMR via FFI": confermato wasmi 0.36.
- ❌ "Host functions: args_get, environ_get, clock_time_get, random_get,
  fd_read/write/seek, path_*, proc_exit" (WASI P1): rimpiazzato dal
  superset WASIX (vedi lista host fns sotto).

## Obiettivo

Far girare un binario `.wasm` reale sotto ruos come embassy task, con
host fns WASIX sufficienti a coprire: lifecycle, stdio, VFS file
operations, clock, random, e TCP sockets su loopback. A fine Step 10:

1. `init.wasm` (welcome banner) carica e stampa sul console (stdout).
2. `server.wasm` apre TCP socket su `127.0.0.1:8080` e accepta.
3. `client.wasm` si connette al server, scambia "ping"/"pong".

Tre `.wasm` distinti, tutti caricati come **Limine modules** e montati
in tmpfs al boot, eseguiti come task embassy concorrenti.

## Non-obiettivi (rimandati a step futuri)

- `proc_fork`/`proc_exec`/`proc_join`/process table (multi-process)
- `thread_spawn`/`futex_*` (multi-thread guest)
- Signals (kill, sigaction)
- TTY ioctl (termios) — Step 12 (PTY)
- DNS resolution / `resolve` host fn — Step 14 (real network)
- IPv6
- Mounts / unionfs
- WASIX binary compat con Wasmer registry (bash.wasm ecc.) — la spec
  ABI è 0.x moving target; rivisiteremo quando arriverà Step 13.

## Decisioni strategiche (brainstorm 2026-05-28)

1. **Runtime**: `wasmi` 0.36, `default-features=false`. Pure Rust, no_std
   + alloc, interprete (no JIT). Performance ridotta, ok per Step 10.
2. **WASM loading**: **Limine modules** (Opt 3 del brainstorm). `limine.conf`
   dichiara N moduli; kernel li monta in tmpfs al loro path al boot. Pattern
   standard Linux-style initramfs.
3. **Sockets**: **smoltcp loopback** (Opt C del brainstorm). Aggiunge stack
   di rete completo (no_std), driver `Loopback`-only ora; NIC reale a Step
   14.
4. **Lifecycle wasm**: ogni `.wasm` gira nel suo embassy task. Host fns
   sync chiamano `embassy_futures::block_on` interno al task per attendere
   I/O async.
5. **CSPRNG**: weak xorshift seedato da TICKS. RDRAND/getentropy a Step 14
   quando arriva crypto SSH (sicurezza vera, non solo qualità RNG).
6. **ABI snapshot**: pinniamo WASIX 0.x ABI come è oggi (2026-05-28),
   documentando in `docs/wasix-abi-snapshot.md`. Quando arriverà Step 13
   (bash) decideremo se aderire alla spec ufficiale o restare proprietari.

## Architettura

```
Boot phase (sync, current):
  Limine → kmain → init → vfs::init
  ↓ Step 10 new:
  modules::mount_all()         // iter ModuleRequest, mount each in tmpfs
  net::init()                   // smoltcp iface + Loopback device
  ↓
  vfs::block_on(boot_smoke)    // existing
  ↓
Steady-state (executor):
  executor::run() {
    spawn tick_task            // existing (Step 9)
    spawn kbd_echo_task        // existing (Step 9)
    spawn net_poll_task        // NEW: iface.poll() ogni 10ms
    spawn wasm_task("/init.wasm")    // NEW
    spawn wasm_task("/server.wasm")  // NEW
    spawn wasm_task("/client.wasm")  // NEW (con Delay::ticks(200) prima
                                     //      di sock_connect per dare
                                     //      tempo al server di ascoltare)
  }
```

`wasm_task` è un embassy task generico parametrizzato dal path nel VFS:

```rust
#[embassy_executor::task(pool_size = 3)]
async fn wasm_task(path: &'static str) {
    let bytes = vfs::read_all(path).await.unwrap();
    let mut runtime = wasm::Runtime::new(bytes).unwrap();
    runtime.run().await;  // blocca finché .wasm chiama proc_exit
    kprintln!("ruos: {} exited cleanly", path);
}
```

Embassy macro `task(pool_size = 3)` ci dà 3 slot statici per istanze
diverse della stessa funzione task. Sufficiente per Step 10.

## Componenti

### Modulo `kernel/src/modules.rs`

API minimale per esporre i moduli Limine al kernel + monto VFS:

```rust
pub struct BootModule {
    pub path: &'static str,
    pub data: &'static [u8],
}

/// Iter su tutti i moduli Limine, monta ciascuno in tmpfs al path
/// dichiarato. Ritorna numero di moduli montati.
pub fn mount_all() -> usize { ... }

/// Accesso diretto allo slice di moduli (read-only, lifetime 'static).
pub fn modules() -> &'static [BootModule] { ... }
```

Logica:
- `limine::ModuleRequest` esposta via `#[used] static MODULES: ModuleRequest`.
- Al `mount_all()`, per ogni modulo: crea il file in tmpfs al path,
  ne copia il contenuto. (Alternativa: zero-copy via riferimento al
  buffer in fisico/HHDM. Decisione in implementazione; copia è
  conservatice e safer.)

### Modulo `kernel/src/wasm/`

```
wasm/
  mod.rs       — Runtime struct, run(), instantiate
  host/
    mod.rs     — namespace WASIX, import resolver
    lifecycle.rs — args_*, environ_*, proc_exit
    fd.rs        — fd_read, fd_write, fd_seek, fd_close, fd_fdstat_*,
                   fd_prestat_* (preopens), fd_dup
    path.rs      — path_open, path_create_directory,
                   path_remove_directory, path_unlink_file,
                   path_filestat_get
    clock.rs     — clock_time_get, clock_res_get
    random.rs    — random_get (xorshift weak prng)
    sock.rs      — sock_open, sock_bind, sock_listen, sock_accept,
                   sock_connect, sock_recv, sock_send, sock_shutdown,
                   sock_status, sock_addr_local, sock_addr_peer
  state.rs     — RuntimeState (per-instance): fd table, sock table,
                 args/env, exit code
```

`Runtime` struct:
- contiene `wasmi::Store`, `wasmi::Linker`, `wasmi::Instance`
- contiene `RuntimeState` come `Store::data()`
- `run().await` chiama `_start` export; quando wasm trapsza con
  `WasiTrap::Exit(code)` (definito da noi), salva exit code e ritorna

Host fn signature pattern (esempio fd_write):

```rust
fn fd_write(
    mut caller: Caller<'_, RuntimeState>,
    fd: u32,
    iovs_ptr: u32,
    iovs_len: u32,
    nwritten_ptr: u32,
) -> Result<u32, Trap> { ... } // ritorna errno
```

Tutte le host fns sync. Quando serve attendere I/O (es. sock_accept,
fd_read da console):
1. Costruisco il future per l'I/O.
2. `embassy_futures::block_on(future)` dentro la host fn, *dal task
   embassy che ospita il wasm*. Il task cede CPU all'executor mentre
   blocca, altri task girano.

Questo richiede che il task embassy che chiama `runtime.run()` sia esso
stesso un async fn — confermato dall'architettura (`wasm_task` è
`#[embassy_executor::task] async fn`).

### Modulo `kernel/src/net/`

```
net/
  mod.rs       — init(), poll_task, interface globale
  loopback.rs  — smoltcp::phy::Loopback wrapper
  sockets.rs   — SocketPool: HashMap<SockHandle, smoltcp::SocketHandle>,
                 wrappers async (accept/connect/recv/send come futures)
```

`net::init()`:
- crea smoltcp `Interface` con device `Loopback`, IP `127.0.0.1/8`
- inizializza `SocketSet` con capacità statica (es. 8 socket)
- pubblica entrambi in un `Mutex` static

`net_poll_task` (embassy task spawnato dall'executor):

```rust
#[embassy_executor::task]
async fn net_poll_task() {
    loop {
        net::poll();   // iface.poll(Instant::from_millis(now), ...)
        delay::Delay::ticks(1).await;   // 10ms cadence
    }
}
```

SocketPool::accept/connect/recv/send sono async wrappers: ritornano
future che fanno polling sullo stato smoltcp + cedono via `yield_now`
finché smoltcp dichiara la socket pronta.

### Lista host fns (~25 totali)

| Modulo | Host fn | Compl. | Note |
|--------|---------|--------|------|
| lifecycle | `args_sizes_get` | 1 | Ritorna count=0/buf_size=0 inizialmente |
| lifecycle | `args_get` | 1 | No-op (no args from kernel side) |
| lifecycle | `environ_sizes_get` | 1 | 0/0 |
| lifecycle | `environ_get` | 1 | No-op |
| lifecycle | `proc_exit` | 1 | Trap con WasiTrap::Exit(code) |
| clock | `clock_time_get` | 2 | TICKS * 10ms → nanos |
| clock | `clock_res_get` | 1 | 10_000_000 ns |
| random | `random_get` | 2 | xorshift, seedato da TICKS al boot |
| fd | `fd_write` | 3 | iovec → VFS write; FD 1/2 → console |
| fd | `fd_read` | 3 | VFS read; FD 0 → keyboard queue |
| fd | `fd_seek` | 2 | VFS seek |
| fd | `fd_close` | 1 | VFS close + FD table cleanup |
| fd | `fd_fdstat_get` | 2 | Tipo file + flags |
| fd | `fd_prestat_get` | 2 | Preopen dir info |
| fd | `fd_prestat_dir_name` | 2 | Preopen dir path string |
| fd | `fd_dup` | 2 | Duplicate FD entry |
| path | `path_open` | 4 | Resolve preopen + relative path → VFS open |
| path | `path_create_directory` | 2 | VFS mkdir |
| path | `path_remove_directory` | 2 | VFS rmdir |
| path | `path_unlink_file` | 2 | VFS unlink |
| path | `path_filestat_get` | 3 | VFS stat |
| sock | `sock_open` | 2 | Alloc TCP socket in pool |
| sock | `sock_bind` | 2 | smoltcp bind |
| sock | `sock_listen` | 2 | smoltcp listen |
| sock | `sock_accept` | 4 | async wait incoming, ritorna nuovo FD |
| sock | `sock_connect` | 4 | async wait connect complete |
| sock | `sock_recv` | 4 | async read TCP bytes |
| sock | `sock_send` | 4 | async write TCP bytes |
| sock | `sock_shutdown` | 1 | smoltcp close |
| sock | `sock_status` | 1 | State enum mapping |
| sock | `sock_addr_local` | 2 | Endpoint info |
| sock | `sock_addr_peer` | 2 | Endpoint info |

"Compl." = scala soggettiva 1-5, indica effort relativo.

### Demo `.wasm` (in repository, build separato)

Nuovo workspace `user/`:

```
user/
  Cargo.toml          — workspace separato (no_std non necessario, ok std)
  init/
    Cargo.toml
    src/main.rs       — welcome banner via println!("...")
  server/
    Cargo.toml
    src/main.rs       — TcpListener bind 127.0.0.1:8080, accept, echo
  client/
    Cargo.toml
    src/main.rs       — Delay 2s, TcpStream connect, send "ping", recv
```

Build via:
```
cd user && cargo build --target wasm32-wasip1 --release
cp target/wasm32-wasip1/release/{init,server,client}.wasm ../user-bin/
```

Makefile della root: target `make user-wasm` builda tutti e tre e copia
in `user-bin/`. `make iso` li include come Limine modules.

### `limine.conf` aggiornata

```
timeout: 0
default_entry: 1

/ruos
    protocol: limine
    kernel_path: boot():/boot/kernel.elf
    module_path: boot():/init.wasm
    module_cmdline: /init.wasm
    module_path: boot():/server.wasm
    module_cmdline: /server.wasm
    module_path: boot():/client.wasm
    module_cmdline: /client.wasm
```

Il `module_cmdline` è il path nel VFS al quale il modulo verrà montato.

## Data flow

### Boot: Limine module → VFS

```
Limine carica kernel + 3 .wasm in RAM, espone ModuleRequest
  ↓
kmain → vfs::init() (tmpfs mount)
  ↓
modules::mount_all():
  per ogni modulo Limine: vfs::create("/init.wasm") + write(bytes)
  ↓
kprintln!("ruos: mounted {n} boot modules")
```

### wasm_task: fd_write to stdout

```
init.wasm calls println!("Welcome to ruos")
  → libc → fd_write(1, iovec, ...)
  → import "wasi_snapshot_preview1::fd_write"
wasmi calls host fd_write(caller, 1, iovs_ptr, iovs_len, nwritten_ptr)
  → read iovec from wasm linear memory
  → for each iov: write bytes to console::CONSOLE
  → return errno=0
wasm continues, eventually calls proc_exit(0)
  → Trap WasiTrap::Exit(0)
wasm_task catches Trap, kprintln!("ruos: init.wasm exited cleanly")
```

### sock_accept wake path

```
server.wasm: listener.accept().unwrap()
  → libc maps to sock_accept(listen_fd, ...)
wasmi calls host sock_accept(caller, listen_fd, ...)
host fn:
  let socket_handle = state.sock_pool.lookup(listen_fd);
  let accept_future = net::SocketPool::accept_future(socket_handle);
  embassy_futures::block_on(accept_future):
    poll:
      iface_state = net::poll();   // already running in net_poll_task too
      if listener has accepted connection:
        return Poll::Ready(new_handle)
      else:
        register waker, Poll::Pending
  → new_handle ottenuto
  → alloca nuovo fd, mappa al new_handle, ritorna fd a wasm
```

Mentre `block_on` aspetta, `executor::run` polla altri task (es.
`net_poll_task` che processa pacchetti smoltcp). Quando il connect del
client arriva, lo waker della future è risvegliato dal `net_poll_task`
stesso.

## Error handling

- Module mount fallisce (out-of-memory in tmpfs) → panic. Step 10 ha
  RAM abbondante.
- Wasm instantiation fail (bad bytecode, missing import) → kprintln
  errore + task termina pulito.
- Host fn errno: ritorna errno appropriato (es. EBADF se FD non
  esiste, EINVAL se param invalido). Conforme spec WASIX errno.
- smoltcp poll panic → propaga a panic kernel (nessun recovery; debug).

## Concurrency / ISR safety

- `net::INTERFACE` e `net::SOCKETS` sono `Mutex` (spin). Lockate da
  `net_poll_task` per `iface.poll()` e dalle host fns sock_* via
  `without_interrupts(|| lock())`.
- `SocketPool` (kernel-side FD table) usa `Mutex`. Idem pattern.
- Wasm instances sono confinate al loro embassy task — no shared
  mutable state tra istanze. State `RuntimeState` è private al task.
- Limine module data resta in `'static` slice (HHDM mappata); read-only,
  zero-locking.

## Testing

### Smoke test automatizzato (`make run-test`)

```
HELLO := ruos: client.wasm: rx='pong'
```

Expected serial log al boot:
```
ruos: vfs init ok mounts=1
ruos: mounted 3 boot modules
ruos: net init ok addr=127.0.0.1/8
ruos: executor up
ruos: async tick=0
╔══════════════════════════════════╗
║         Welcome to ruos          ║
║   wasm32-wasip1 / WASIX host     ║
╚══════════════════════════════════╝
ruos: init.wasm exited cleanly
ruos: server.wasm: listening on 127.0.0.1:8080
ruos: server.wasm: accepted
ruos: client.wasm: tx='ping'
ruos: server.wasm: rx='ping' tx='pong'
ruos: client.wasm: rx='pong'
ruos: client.wasm exited cleanly
ruos: server.wasm exited cleanly
```

Note di scheduling: i 3 wasm task girano interleaved. Il client aspetta
200ms (`Delay::ticks(20)`) prima di connettersi per assicurare che il
server sia in `listen`. Sequenze esatte di log possono interleavare
con `async tick=N`.

### Regression checks

- `make build` clean (warning count baseline 12 ± wasm/smoltcp additions)
- Cursor blink intatto (Step 8 invariant)
- VFS smoke intatto (Step 7)
- async tick + kbd echo intatti (Step 9)

### Manual smoke

Stessa procedura Step 9: VBox con I/O APIC, vedo banner welcome +
log socket + premo tasti → kbd echo continua a funzionare.

## File toccati (riepilogo)

**Nuovi:**
- `kernel/src/modules.rs`
- `kernel/src/wasm/mod.rs` + `wasm/state.rs` + `wasm/host/*.rs`
- `kernel/src/net/{mod,loopback,sockets}.rs`
- `user/Cargo.toml` + `user/{init,server,client}/{Cargo.toml,src/main.rs}`
- `user-bin/{init,server,client}.wasm` (build output)
- `docs/wasix-abi-snapshot.md` (versione ABI pinned)

**Modificati:**
- `kernel/Cargo.toml` (+wasmi, +smoltcp, +embassy-futures)
- `kernel/src/main.rs` (mod modules; mod wasm; mod net; init order)
- `kernel/src/executor/mod.rs` (spawn net_poll_task + 3 wasm_task)
- `limine.conf` (module_path × 3)
- `Makefile` (target `user-wasm` + iso include moduli + HELLO sentinel)

## Open points (decisi in implementazione)

- **Zero-copy module data**: copy da Limine memmap a tmpfs (semplice)
  vs slice diretto (zero-copy, ma serve vfs read-only file backed da
  slice). Partire con copia; ottimizzare se serve.
- **wasmi memory access**: chiamate `caller.get_export("memory")` ogni
  host fn vs cache nello `Store::data()`. Decisione su benchmark.
- **embassy_futures::block_on** vs custom block-from-task: validare
  che block_on di embassy-futures funzioni mentre il task chiamante è
  on-CPU dell'executor; sennò roll-our-own che usa il waker
  dell'executor ambient.
- **smoltcp socket buffer size**: 4 KB per direzione partire. Tunabile.
- **Numero pool wasm_task**: `pool_size = 3` per Step 10. Aumentare
  quando bash + coreutils + altri coesisteranno.
