# 67 — Followups tracciati per Step 10

**Data:** 2026-05-28

## Cosa

Creato `docs/followups/step-10.md` con i followup emersi dai per-task
review + whole-implementation discussion:

- **F-MAJOR**: Step 10.5 = green-threads / fiber pattern via
  `wasmi::Func::call_resumable` + embassy Future. Risolve la
  limitazione architetturale che ha richiesto `setup_demo_sockets`
  (pre-handshake + pre-pong) a Task 6.
- F1: rimuovere `setup_demo_sockets` quando F-MAJOR chiude.
- F2: retirement di `recv_sync`/`send_sync` post-F-MAJOR.
- F3: doc ABI WASIX subset nostro.
- F4: socket buffer size tuning.
- F5: `path_*` directory ops ENOSYS → reali (~20 LoC each).
- F6: aggiungere link alias `wasix_32v1` accanto a `wasi_snapshot_preview1`.
- F7: valutare AOT compilazione wasmi per Step 11+.

L'F-MAJOR è documentato con architettura nera-su-bianco:
- `Fiber` struct sostituisce `Runtime`
- `pub async fn run(&mut self)` invece di sync `run`
- Host fns I/O ritornano `Err(SuspendReason::*)` invece di `block_on`
- Loop esterno: `state.host_error().downcast_ref::<SuspendReason>()`
  → await su corrispondente future → `state.resume(...)` → repeat

## Perché

Mirror del pattern usato per Step 8/9 (`docs/followups/step-{8,9}.md`).
F-MAJOR è tracciato come "Step 10.5 a sé", non bloccante per chiudere
Step 10. Gli altri sono cleanup standard. Tutto leggibile prima di
iniziare Step 11.

## File toccati

- docs/followups/step-10.md (nuovo)
- CHANGELOG/67-26-05-28-step10-followups.md (nuovo)
