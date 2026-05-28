# Step 10.5 — WASIX fibers (green-threads via `Func::call_resumable`)

**Data:** 2026-05-28
**Roadmap step:** 10.5 (filled in dopo che il pivot WASIX di Step 10 ha
incontrato il limite di `embassy_futures::block_on`)
**Stato:** spec approvata, da implementare

## Contesto

Step 10 ha portato su wasmi 1.0.9 + ~25 host fns WASIX + smoltcp
loopback. Limite architetturale emerso a Task 6: le host fns sync che
chiamano `embassy_futures::block_on(future)` **busy-pollano** il
future senza cedere CPU all'executor embassy. Due wasm task non
possono comunicare cooperativamente via TCP: il primo che chiama
`sock_accept`/`sock_connect` blocca anche il secondo.

Workaround attuale (`setup_demo_sockets`): kernel pre-stabilisce
3-way handshake + pre-popola `"pong"` lato client *prima* che
l'executor parta. Il sentinel `client.wasm: rx='pong'` passa ma il
roundtrip è in parte mediato dal kernel (vedi `docs/followups/step-10.md`
F-MAJOR per dettagli).

## Obiettivo

Implementare **green-threads / fiber pattern**:

- Ogni istanza wasmi = un **fiber** (continuation salvabile).
- Le WASIX host fns I/O = **punti di yield** che restituiscono `Err(SuspendReason::*)`.
- `wasmi::Func::call_resumable` (già in wasmi 1.0.9) cattura lo stato.
- Loop esterno (in `Fiber::run`, async) decodifica `SuspendReason`,
  `.await` la future giusta, chiama `state.resume(...)`.
- Embassy executor = multiplexer.

Effetto: due wasm task possono scambiare TCP **realmente** senza
pre-loading; quando uno blocca su `sock_recv`, l'altro gira.

## Decisioni strategiche (brainstorm 2026-05-28)

1. **Migration scope**: **Opt A** — tutti gli I/O host fns migrano a
   `SuspendReason`, non solo `sock_*`. Coerenza architetturale: nessun
   mix sync/async. Anche se `fd_read` VFS è oggi istantaneo (tmpfs),
   il pattern resta uniforme e si prepara per Step 14+ (disk-backed VFS).
2. **Smoke contract finale**: stesso sentinel `client.wasm: rx='pong'`
   ma **vero** (senza pre-loading). Aggiunto log kernel
   `ruos: real ping-pong (no preload)` come asserzione negativa.
3. **Decomposizione 3 task** (vedi sotto): Sleep first (warmup), poi
   sock_*, poi fd_*/path_*.

## Architettura

### `kernel/src/wasm/suspend.rs`

```rust
//! Yield points per le host fns I/O. Restituite come `HostError` da
//! `Err(SuspendReason::*)` invece di `block_on`. Il loop esterno
//! `Fiber::run` le decodifica e await la future corrispondente.

use alloc::vec::Vec;
use alloc::string::String;
use smoltcp::iface::SocketHandle;
use smoltcp::wire::IpEndpoint;

#[derive(Debug)]
pub enum SuspendReason {
    Sleep { ticks: u64 },

    SockAccept { handle: SocketHandle },
    SockConnect { handle: SocketHandle, remote: IpEndpoint, local_port: u16 },
    SockRecv { handle: SocketHandle, max_len: usize },
    SockSend { handle: SocketHandle, bytes: Vec<u8> },

    VfsRead { fd: crate::vfs::Fd, max_len: usize },
    VfsWrite { fd: crate::vfs::Fd, bytes: Vec<u8> },
    VfsSeek { fd: crate::vfs::Fd, offset: i64, whence: crate::vfs::Whence },
    VfsClose { fd: crate::vfs::Fd },
    PathOpen { path: String, flags: crate::vfs::OpenFlags },

    KbdReadChar,
}

impl core::fmt::Display for SuspendReason {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl wasmi::core::HostError for SuspendReason {}

#[derive(Debug)]
pub enum ResumeValue {
    None,
    Errno(i32),
    Bytes(Vec<u8>),
    Fd(crate::vfs::Fd),
    NewSocketIdx(usize),  // per sock_accept che ritorna nuovo socket
    BytesWritten(usize),
    BytesRead(Vec<u8>),
    Seek(u64),
    Byte(u8),  // per kbd_read_char
}
```

### `kernel/src/wasm/fiber.rs`

```rust
use wasmi::{Engine, Module, Store, Linker, Instance, ResumableInvocation, Val};
use crate::wasm::state::RuntimeState;
use crate::wasm::host;
use crate::wasm::suspend::{SuspendReason, ResumeValue};
use crate::kprintln;

pub struct Fiber {
    store: Store<RuntimeState>,
    instance: Instance,
}

impl Fiber {
    pub fn new(bytes: &[u8]) -> Result<Self, wasmi::Error> {
        let engine = Engine::default();
        let module = Module::new(&engine, bytes)?;
        let mut store: Store<RuntimeState> = Store::new(&engine, RuntimeState::new());
        let mut linker: Linker<RuntimeState> = Linker::new(&engine);
        host::install(&mut linker)?;
        let instance = linker.instantiate_and_start(&mut store, &module)?;
        Ok(Self { store, instance })
    }

    pub async fn run(&mut self) -> i32 {
        let start = match self.instance.get_typed_func::<(), ()>(&self.store, "_start") {
            Ok(f) => f,
            Err(e) => {
                kprintln!("ruos: wasm: no _start export: {}", e);
                return -1;
            }
        };
        // Convert TypedFunc → Func for call_resumable (resumable API
        // takes &[Val] / &mut [Val]).
        let start = start.func();

        let mut results: [Val; 0] = [];
        let mut inv = match start.call_resumable(&mut self.store, &[], &mut results) {
            Ok(i) => i,
            Err(e) => return Self::error_to_exit(&e),
        };

        loop {
            match inv {
                ResumableInvocation::Finished(_) => return 0,
                ResumableInvocation::Resumable(state) => {
                    let reason: SuspendReason = state.host_error()
                        .downcast_ref::<SuspendReason>()
                        .cloned()   // Reason needs Clone, or rebuild from refs
                        .expect("host trap must carry SuspendReason");
                    let resume_val = self.dispatch(reason).await;
                    let resume_args = Self::encode_resume(resume_val);
                    let mut next_results: [Val; 0] = [];
                    inv = match state.resume(&mut self.store, &resume_args, &mut next_results) {
                        Ok(i) => i,
                        Err(e) => return Self::error_to_exit(&e),
                    };
                }
            }
        }
    }

    async fn dispatch(&mut self, reason: SuspendReason) -> ResumeValue {
        use crate::net::sockets as net_sock;
        match reason {
            SuspendReason::Sleep { ticks } => {
                crate::executor::delay::Delay::ticks(ticks).await;
                ResumeValue::None
            }
            SuspendReason::SockAccept { handle } => {
                match net_sock::accept(handle).await {
                    Ok(()) => ResumeValue::None,
                    Err(_) => ResumeValue::Errno(8),
                }
            }
            SuspendReason::SockConnect { handle, remote, local_port } => {
                match net_sock::connect(handle, remote, local_port).await {
                    Ok(()) => ResumeValue::None,
                    Err(_) => ResumeValue::Errno(8),
                }
            }
            SuspendReason::SockRecv { handle, max_len } => {
                let mut buf = alloc::vec![0u8; max_len];
                match net_sock::recv(handle, &mut buf).await {
                    Ok(n) => { buf.truncate(n); ResumeValue::BytesRead(buf) }
                    Err(_) => ResumeValue::Errno(8),
                }
            }
            SuspendReason::SockSend { handle, bytes } => {
                match net_sock::send(handle, &bytes).await {
                    Ok(n) => ResumeValue::BytesWritten(n),
                    Err(_) => ResumeValue::Errno(8),
                }
            }
            SuspendReason::VfsRead { fd, max_len } => {
                let mut buf = alloc::vec![0u8; max_len];
                match crate::vfs::read(fd, &mut buf).await {
                    Ok(n) => { buf.truncate(n); ResumeValue::BytesRead(buf) }
                    Err(_) => ResumeValue::Errno(8),
                }
            }
            SuspendReason::VfsWrite { fd, bytes } => {
                match crate::vfs::write(fd, &bytes).await {
                    Ok(n) => ResumeValue::BytesWritten(n),
                    Err(_) => ResumeValue::Errno(8),
                }
            }
            SuspendReason::VfsSeek { fd, offset, whence } => {
                match crate::vfs::seek(fd, offset, whence).await {
                    Ok(n) => ResumeValue::Seek(n),
                    Err(_) => ResumeValue::Errno(8),
                }
            }
            SuspendReason::VfsClose { fd } => {
                let _ = crate::vfs::close(fd).await;
                ResumeValue::None
            }
            SuspendReason::PathOpen { path, flags } => {
                match crate::vfs::open(&path, flags).await {
                    Ok(fd) => ResumeValue::Fd(fd),
                    Err(_) => ResumeValue::Errno(8),
                }
            }
            SuspendReason::KbdReadChar => {
                let b = crate::keyboard::queue::read_char().await;
                ResumeValue::Byte(b)
            }
        }
    }

    fn encode_resume(v: ResumeValue) -> Vec<Val> {
        // L'host fn typed signature in wasmi richiede di passare i
        // valori che la call_resumable si aspetta come "ritorno" della
        // host fn che aveva trappato. Per la maggior parte delle host
        // fns errno-style: un singolo i32 = errno (0 = success).
        match v {
            ResumeValue::None => alloc::vec![Val::I32(0)],
            ResumeValue::Errno(n) => alloc::vec![Val::I32(n)],
            ResumeValue::Byte(_)
            | ResumeValue::BytesRead(_)
            | ResumeValue::BytesWritten(_)
            | ResumeValue::Seek(_)
            | ResumeValue::Fd(_)
            | ResumeValue::NewSocketIdx(_) => {
                // Per questi, la host fn deve aver fatto la scrittura
                // dei dati in wasm memory PRIMA di trappare (es.
                // sock_send legge bytes ed esce; ResumeValue::BytesWritten
                // codifica solo il count).
                //
                // Vedi sezione "Pattern: dati in memory, errno on resume"
                // sotto.
                alloc::vec![Val::I32(0)]
            }
        }
    }

    fn error_to_exit(e: &wasmi::Error) -> i32 {
        if let Some(code) = e.kind().as_i32_exit_status() {
            return code;
        }
        kprintln!("ruos: wasm trap: {}", e);
        -1
    }
}
```

### Pattern: dati in memory, errno on resume

Le WASIX host fns hanno tipicamente questa shape:

```
sock_recv(fd, buf_ptr, buf_len, nrecv_ptr) -> errno
```

Il dato viene scritto in wasm memory (a `buf_ptr`), il count va in
`nrecv_ptr`, e il *return value* è solo errno. Implementazione fiber:

1. Host fn `sock_recv` viene chiamata: legge `buf_ptr`/`buf_len`/`nrecv_ptr`
   dai params, **NON** scrive ancora in memoria, trapsza con:
   ```rust
   Err(SuspendReason::SockRecv { handle, max_len: buf_len })
   ```
2. Fiber loop riceve il trap, decodifica `SockRecv`, await `net::sockets::recv`.
3. Resume value = `ResumeValue::BytesRead(buf)`.
4. **Prima** di chiamare `state.resume`, il loop esterno scrive `buf` in
   wasm memory a `buf_ptr`, scrive `n` in `nrecv_ptr`.
5. `state.resume` con `[Val::I32(0)]` (errno=0); il wasm vede il `Ok(0)`
   di ritorno dalla sua chiamata host fn, legge buf da memoria.

**Problema**: il loop esterno non ha più accesso a `buf_ptr` e `nrecv_ptr`
(li ha solo la host fn quando trapsza). Soluzione: includere quei puntatori
in `SuspendReason::SockRecv`:

```rust
SuspendReason::SockRecv {
    handle: SocketHandle,
    buf_ptr: u32,
    max_len: usize,
    nrecv_ptr: u32,
}
```

E il dispatch fa:
```rust
SuspendReason::SockRecv { handle, buf_ptr, max_len, nrecv_ptr } => {
    let mut buf = alloc::vec![0u8; max_len];
    let n = net_sock::recv(handle, &mut buf).await?;
    // Write to wasm memory via Caller... ma Caller non c'è più qui!
}
```

**Problema #2**: il dispatch dopo il resume non ha più il `Caller`. Solo
lo `store`. Soluzione: usare `Store::data_mut` (ma quello è
`RuntimeState`, non l'instance memory) — serve **`instance.get_export(&store, "memory").into_memory().unwrap()`** per accedere alla memoria:

```rust
fn write_to_memory(&mut self, ptr: u32, bytes: &[u8]) -> Result<(), wasmi::Error> {
    let mem = self.instance
        .get_export(&self.store, "memory")
        .and_then(|e| e.into_memory())
        .ok_or(wasmi::Error::new("no memory"))?;
    mem.write(&mut self.store, ptr as usize, bytes)
        .map_err(|_| wasmi::Error::new("memory write failed"))
}
```

Pattern finale:

```rust
SuspendReason::SockRecv { handle, buf_ptr, max_len, nrecv_ptr } => {
    let mut buf = alloc::vec![0u8; max_len];
    match net_sock::recv(handle, &mut buf).await {
        Ok(n) => {
            self.write_to_memory(buf_ptr, &buf[..n]).ok();
            self.write_u32(nrecv_ptr, n as u32).ok();
            ResumeValue::Errno(0)
        }
        Err(_) => ResumeValue::Errno(8),
    }
}
```

Le altre host fns hanno pattern simile (passare `*_ptr` nel SuspendReason).

### Demo cleanup (Task 2)

Drop in `kernel/src/wasm/mod.rs`:
```rust
pub fn setup_demo_sockets() { ... }
pub static SERVER_SOCK_IDX: ...;
pub static CLIENT_SOCK_IDX: ...;
```

Drop in `kernel/src/main.rs`:
```rust
wasm::setup_demo_sockets();
```

Drop in `kernel/src/net/sockets.rs`:
```rust
pub fn connect_sync(...) { ... }
pub fn accept_sync(...) { ... }
pub fn recv_sync(...) { ... }
pub fn send_sync(...) { ... }
```

`server.wasm` e `client.wasm` non vengono modificati — chiamano già
le WASIX host fns che adesso vanno via SuspendReason.

Aggiunto log al boot kernel-side:
```rust
kprintln!("ruos: real ping-pong (no preload)");
```
chiamato in `kmain` dopo `net::init()` per asserire visivamente che il
hack è andato.

### Diagramma flow Task 2 (verifica architetturale)

```
T0:  client.wasm task scheduled
     sock_connect host fn → trap SockConnect
     Fiber::run dispatches → net::sockets::connect(...).await
       smoltcp: SYN sent (loopback driver)
       connect future: Poll::Pending → yield to executor
T1:  embassy executor picks net_poll_task
     net_poll_task: iface.poll() → smoltcp processes SYN, sends SYN-ACK
     net_poll_task: Delay::ticks(1).await → yield
T2:  executor picks server.wasm
     sock_accept host fn → trap SockAccept
     Fiber::run dispatches → net::sockets::accept(...).await
       check_state(Established) → Yes (ACK + 3way done)
       Poll::Ready
     state.resume → server.wasm continues, calls sock_recv
       trap SockRecv → await recv (no data yet) → Pending → yield
T3:  executor picks client.wasm
     connect future: check_state(Established) → Yes
     state.resume(errno=0) → client.wasm continues, calls sock_send "ping"
       trap SockSend → await send → smoltcp queues bytes
       Poll::Ready → state.resume → client.wasm continues, calls sock_recv
       trap SockRecv → await recv (no data yet) → Pending → yield
T4:  net_poll_task: iface.poll() → "ping" moves from client TX to server RX
T5:  executor picks server.wasm
     recv future check_state(can_recv) → Yes → returns "ping"
     state.resume(errno=0) → server.wasm reads "ping" in its buffer,
       calls sock_send "pong"
       trap SockSend → await send → smoltcp queues "pong"
T6:  net_poll_task: iface.poll() → "pong" moves to client RX
T7:  executor picks client.wasm
     recv future: Yes "pong"
     state.resume(errno=0) → client.wasm reads "pong"
     prints "client.wasm: rx='pong'"   ← SENTINEL (real)
```

Tutto cooperative. Nessun pre-loading.

## Smoke test

`Makefile` HELLO:
- Step 10 finale: `client.wasm: rx='pong'`
- Step 10.5 finale: identico, MA il significato cambia (vero roundtrip).

Test negativo aggiunto kernel-side: log `ruos: real ping-pong (no preload)`
visibile nella prima parte del boot prove che `setup_demo_sockets` non
esiste più (compile-time: il simbolo è stato rimosso).

## Componenti / file toccati (riepilogo)

**Nuovi:**
- `kernel/src/wasm/suspend.rs`
- `kernel/src/wasm/fiber.rs`

**Modificati:**
- `kernel/src/wasm/mod.rs` — drop `Runtime`, drop `setup_demo_sockets`,
  `run_at` usa `Fiber`. Add `ruos: real ping-pong (no preload)` log.
- `kernel/src/wasm/host/{lifecycle,fd,path,sock,clock,random}.rs` —
  tutte le host fns I/O ritornano `Err(SuspendReason::*)`. Le sync
  (clock, random, console fd_write, proc_exit) restano sync.
- `kernel/src/net/sockets.rs` — drop `*_sync` wrappers.
- `kernel/src/main.rs` — drop `setup_demo_sockets` call.

## Decomposizione 3 task

1. **T1 — Fiber scaffolding + Sleep migration**: nuovi `fiber.rs` +
   `suspend.rs` con sola variante `Sleep`. `run_at` switcha da
   `Runtime::run` (sync) a `Fiber::run` (async). Migra solo una host
   fn: aggiungi `wasi_snapshot_preview1::poll_oneoff` (= Sleep) per
   permettere a init.wasm di chiamare `thread::sleep`. Smoke contract:
   `async tick=N` continuano a girare DURANTE il sleep del wasm
   (cooperative proof).
2. **T2 — Migrate sock_* + drop pre-loading**: tutte le sock_* a
   SuspendReason. Drop `setup_demo_sockets` + sync wrappers in
   `sockets.rs`. Add `ruos: real ping-pong (no preload)` log al boot.
   Smoke: stesso sentinel `client.wasm: rx='pong'`, vero.
3. **T3 — Migrate fd_* + path_*** Final cleanup. Tutto a SuspendReason.
   Verifica build clean + run-test PASS + warning count baseline.

## Out of scope

- Drop `embassy_futures` dep (potrebbe restare per usi interni come
  block_on init-time fuori dall'executor)
- F4-F7 di `docs/followups/step-10.md` (cleanup separati)
- Migrazione lifecycle host fns (args_get, proc_exit, ecc.) — restano
  sync, sono istantanei e non bloccano
