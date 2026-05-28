# Step 10 — followups

Followup emersi dai per-task review + dal whole-implementation
discussion di Step 10 (WASIX bootstrap). Aperti al merge di
`feature/wasix-bootstrap` → `main`. **Uno è MAJOR** (Step 10.5 a sé);
gli altri sono cleanup standard.

## F-MAJOR — Step 10.5: green-threads / fiber pattern per cooperative wasm async

**File:** `kernel/src/wasm/mod.rs`, `kernel/src/wasm/host/sock.rs`,
`kernel/src/net/sockets.rs`
**Severity:** 🟠 architecturally significant, da fare prima di Step 11

### Problema

Lo Step 10 ha tre wasm task concorrenti (`init.wasm`, `server.wasm`,
`client.wasm`) ciascuno wrappato in un `#[embassy_executor::task]`. Le
WASIX host fns sync che fanno I/O usano `embassy_futures::block_on`
per attendere future async (es. `crate::net::sockets::recv`).

**`embassy_futures::block_on` busy-polla la future**: non cede CPU
all'executor durante il poll. Conseguenza: un wasm task in attesa I/O
blocca anche tutti gli altri wasm task. Due wasm che provano a
comunicare via TCP loopback si deadlock-ano: il server aspetta
`accept`, il client aspetta `connect`; nessuno dei due cede.

Workaround applicato a Task 6: `setup_demo_sockets()` pre-stabilisce
il 3-way handshake **prima** che l'executor parta, e pre-popola
`"pong"` nel buffer RX del client. Il sentinel `client.wasm: rx='pong'`
passa, ma il "demo" è solo parzialmente reale:

- `client→server "ping"` reale via smoltcp loopback ✓
- `server→client "pong"` fake (pre-popolato dal kernel) ✗

### Soluzione: green threads / fiber pattern

Origine concettuale: GNU Pth → Erlang processes → Go goroutines →
Lua coroutines → Python gevent → Tokio `spawn`. Il runtime offre
"thread leggeri" che il programmatore tratta come thread normali
(I/O scritto in stile sequenziale); il runtime li multiplexa su pochi
thread reali via *suspensione automatica* sulle operazioni bloccanti.

**Mappatura al nostro caso**:

- Ogni istanza wasmi = **un fiber**.
- Le WASIX host fns I/O = **i punti di sospensione automatica**.
- L'executor embassy = **il multiplexer**.
- La primitiva di yield/resume = **`wasmi::Func::call_resumable`**
  (esiste già in wasmi 1.0.9).

### Architettura proposta

```rust
// wasm/mod.rs (rewrite)
pub struct Fiber {
    store: Store<RuntimeState>,
    state: Option<ResumableInvocation>,
}

impl Fiber {
    pub fn new(bytes: &[u8]) -> Result<Self, Error> { ... }

    /// Async run loop: drive the wasm until it traps on a
    /// HostError carrying a SuspendReason; await the corresponding
    /// future; resume the wasm with the result. Repeat until done.
    pub async fn run(&mut self) -> i32 {
        let start = self.instance.get_typed_func::<(), ()>("_start")?;
        let mut invocation = start.call_resumable(&mut self.store, &[], &mut [])?;
        loop {
            match invocation {
                ResumableInvocation::Finished(_) => return self.exit_code(),
                ResumableInvocation::Resumable(state) => {
                    // Extract SuspendReason from the host error.
                    let reason = state.host_error().downcast_ref::<SuspendReason>();
                    let result = match reason {
                        SuspendReason::SockRecv(handle, len) => {
                            // Await on the smoltcp recv future, cooperatively
                            // yielding to embassy executor.
                            let buf = crate::net::sockets::recv_async(*handle, *len).await;
                            // Encode result into wasm-call return value.
                            ResumeValue::I32(buf.len() as i32)
                        }
                        SuspendReason::SockSend(handle, bytes) => {
                            crate::net::sockets::send_async(*handle, bytes).await;
                            ResumeValue::I32(bytes.len() as i32)
                        }
                        SuspendReason::Sleep(ticks) => {
                            crate::executor::delay::Delay::ticks(*ticks).await;
                            ResumeValue::I32(0)
                        }
                        // ... altri host fns I/O ...
                    };
                    invocation = state.resume(&mut self.store, &[result], &mut [])?;
                }
            }
        }
    }
}

// host/sock.rs (rewrite delle host fns I/O)
fn sock_recv(...) -> Result<i32, Error> {
    // Invece di block_on(...), trap con SuspendReason:
    Err(Error::host(SuspendReason::SockRecv(handle, buf_len)))
}
```

### Cosa cambia rispetto allo stato attuale

| Aspetto | Oggi (Step 10) | Dopo Step 10.5 |
|--------|----------------|----------------|
| Concorrenza wasm | Bloccata da `block_on`; un task alla volta | Cooperative; N task interleaved |
| Host fns I/O | Sync, busy-polling | Trap + resume con value |
| `setup_demo_sockets` | Necessario | Si elimina (kernel non pre-fa nulla) |
| `recv_sync`/`send_sync` | Per i wasm task | Resta per chiamate kernel-side |
| `recv_async`/`send_async` | (esistono parzialmente) | Diventano il path principale |

### Stato wasmi 1.0.9 API

- `Func::call_resumable(&mut store, &[Value], &mut [Value]) -> Result<ResumableInvocation, Error>` ✓
- `ResumableInvocation::{Finished, Resumable(state)}` ✓
- `state.resume(&mut store, &[Value], &mut [Value]) -> Result<ResumableInvocation, Error>` ✓
- `state.host_error() -> Option<&dyn HostError>` ✓

Tutta la primitiva c'è. Si tratta di rewrite di `wasm/mod.rs::Runtime`
da `pub fn run(&mut self) -> i32` (sync) a `pub async fn run(&mut self)
-> i32` (async, drives resume).

### Stima effort

~2 settimane (rewrite Runtime + tutte le host fns I/O + verifica con
demo server/client reale senza pre-loading).

### Step ordering

- Step 10 chiude come-è (sentinel met, hack documentato).
- Step 10.5 dedicato a fiber rewrite.
- Step 11 (era "shell") procede sopra il nuovo Runtime.

---

## F1 — `setup_demo_sockets` da rimuovere

**File:** `kernel/src/wasm/mod.rs`
**Severity:** 🟡 hack da rimuovere

Quando Step 10.5 chiude, `setup_demo_sockets()` e la pre-popolazione di
`"pong"` diventano morti. Eliminare l'intero blocco + i due
`SERVER_SOCK_IDX`/`CLIENT_SOCK_IDX` statici, sostituire con allocazione
ordinaria dei socket dentro le rispettive host fns wasm.

## F2 — `recv_sync`/`send_sync` retirement

**File:** `kernel/src/net/sockets.rs`
**Severity:** 🟡 cleanup

Dopo F-MAJOR, le sync wrappers in `sockets.rs` (`connect_sync`,
`accept_sync`, `recv_sync`, `send_sync`) servono solo a
`setup_demo_sockets`. Eliminare quando F-MAJOR chiude. Mantenere solo
le versioni async.

## F3 — Doc dell'ABI WASIX subset

**File:** `docs/wasix-abi-snapshot.md` (creare)
**Severity:** 🟡 documentation

Lo spec promise va dato in nero su bianco: quale subset WASIX abbiamo
implementato, con quali signature. Importante per debugging quando
arriveranno binari pre-built (bash.wasm) e qualcosa fallirà
all'import resolution.

## F4 — Socket buffer size tuning

**File:** `kernel/src/net/sockets.rs` (`BUF_SIZE = 4096`)
**Severity:** 🟢 nit

4 KB per direzione è arbitrario. Per Step 10 va, per `bash`/`vim`/SSH
potrebbe servire più (16-32 KB). Tunable via const.

## F5 — `path_*` directory operations stub

**File:** `kernel/src/wasm/host/path.rs`
**Severity:** 🟡 ENOSYS stubs

`path_create_directory`/`path_remove_directory`/`path_filestat_get`
ritornano ENOSYS. tmpfs li supporta. Reali sono ~20 LoC ciascuno.

## F6 — `wasi_snapshot_preview1` vs `wasix_32v1` ABI

**File:** `kernel/src/wasm/host/*.rs`
**Severity:** 🟡 future-compat

Oggi tutti `func_wrap` puntano a `"wasi_snapshot_preview1"`. Quando
arriveranno binari built `wasm32-wasix`, useranno `"wasix_32v1"` come
import module. Dovremo registrare le stesse host fns anche su
`wasix_32v1` (alias di link, no codice nuovo).

## F7 — Test coverage `init.wasm` size

**File:** `user/init/src/main.rs`
**Severity:** 🟢 osservazione

`init.wasm` ora è ~150 KB (banner + getrandom + std::fs). Wasmi
interpreter parsa il modulo ad ogni boot. Per Step 11 valutare se
val la pena pre-compilare AOT.
