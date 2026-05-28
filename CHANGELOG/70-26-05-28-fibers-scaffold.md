# 70 — Fiber scaffolding + SuspendReason + poll_oneoff(Sleep)

**Data:** 2026-05-28

## Cosa

Step 10.5 Task 1: architettura fiber per scheduling cooperativo WASM.

- **`kernel/src/wasm/suspend.rs`** — enum `SuspendReason` (Sleep, SockAccept,
  SockConnect, SockRecv, SockSend, VfsRead, VfsWrite, VfsSeek, VfsClose,
  PathOpen, KbdReadChar). Implementa `wasmi::errors::HostError` (via
  `wasmi_core::HostError`). Clone + Debug derivati.

- **`kernel/src/wasm/fiber.rs`** — struct `Fiber` con `async fn run()` basata
  su `Func::call_resumable`. Il loop principale gestisce `ResumableCall::Finished`
  / `HostTrap(ResumableCallHostTrap)` / `OutOfFuel`. Per ogni `HostTrap`
  estrae il `SuspendReason` via `host_error().downcast_ref::<SuspendReason>()`,
  awaita il future corrispondente in `dispatch()`, scrive i risultati in memoria
  wasm, poi chiama `state.resume(errno)`. Compilation mode: `Eager` per evitare
  lazy translation che blocca il cooperative executor.

- **`kernel/src/wasm/mod.rs`** — `run_at` aggiornata ad usare `Fiber::new` +
  `fb.run().await`. `Runtime` struct e `setup_demo_sockets` mantenuti per
  compatibilità Task 2/3. Moduli `suspend` e `fiber` aggiunti.

- **`kernel/src/wasm/host/lifecycle.rs`** — aggiunta `poll_oneoff` host fn che
  parsa la prima subscription WASI, valida che sia di tipo CLOCK (tag=0),
  calcola delta-tick (100 Hz, 1 tick = 10 ms) e ritorna
  `Err(Error::host(SuspendReason::Sleep { ticks, events_ptr, nevents_ptr }))`.
  Registrata nel linker come `wasi_snapshot_preview1::poll_oneoff`.

- **`user/init/src/main.rs`** — aggiunto `thread::sleep(Duration::from_millis(500))`
  dopo il welcome banner; stampa `init.wasm: slept ok` al risveglio.

- **`Makefile`** — HELLO aggiornato a `init.wasm: slept ok`.

## Adattamenti API wasmi 1.0.9 (rispetto al piano)

Il piano usava `ResumableInvocation` — nome inesistente in wasmi 1.0.9.
API reale verificata da source in `/root/.cargo/registry/src/`:

| Piano (ipotetico)               | Reale wasmi 1.0.9                          |
|---------------------------------|--------------------------------------------|
| `ResumableInvocation`           | `ResumableCall`                            |
| `ResumableInvocation::Finished` | `ResumableCall::Finished`                  |
| `ResumableInvocation::Resumable(state)` | `ResumableCall::HostTrap(ResumableCallHostTrap)` + `ResumableCall::OutOfFuel(ResumableCallOutOfFuel)` |
| `state.host_error() -> Option<&dyn HostError>` | `state.host_error() -> &Error`, poi `downcast_ref::<SuspendReason>()` |
| `state.resume(ctx, inputs, outputs)` | `state.resume(ctx, inputs, outputs) -> Result<ResumableCall, Error>` |
| `wasmi::Error::new(...)` | `wasmi::Error::new(...)` — ok, invariato |
| `wasmi::errors::HostError` | `wasmi::errors::HostError` — ok, re-export da wasmi_core |

## Perché

Fix al busy-polling di Step 10: `embassy_futures::block_on` dentro host fn
non cedeva mai il controllo all'executor. Il pattern `call_resumable` + trap
`SuspendReason` permette alla fiber WASM di sospendersi cooperativamente durante
I/O, cedendo il controllo ad altri task async nel frattempo.

## Verifica

```
TEST_PASS
ruos: wasm fiber: suspend Sleep { ticks: 50, ... }
ruos: wasm fiber: sleeping 50 ticks
ruos: executor up            ← executor attivo DURANTE il sleep
ruos: wasm fiber: sleep done, writing 1 event
init.wasm: slept ok
```

## File toccati

- `kernel/src/wasm/suspend.rs` (nuovo)
- `kernel/src/wasm/fiber.rs` (nuovo)
- `kernel/src/wasm/mod.rs`
- `kernel/src/wasm/host/lifecycle.rs`
- `user/init/src/main.rs`
- `user-bin/init.wasm` (ricompilato)
- `Makefile`
- `CHANGELOG/70-26-05-28-fibers-scaffold.md` (questo file)
