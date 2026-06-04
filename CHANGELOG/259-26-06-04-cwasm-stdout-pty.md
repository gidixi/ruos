# 259 — `.cwasm` stdout/stderr bound to the PTY

**Data:** 2026-06-04

## Cosa
Lo stdout/stderr dei tool `.cwasm` (Wasmtime) ora va al PTY del chiamante invece
che direttamente a CONSOLE:
- `WtState.stdout_pty: Option<vfs::Fd>`; `run_cwasm(cwasm, args, pts)` apre
  `/dev/pts/<n>` (WRITE) quando `pts = Some(n)` e lo chiude a fine esecuzione.
- `fd_write` (fd 1/2): se `stdout_pty` è settato → `vfs::write` sul PTY slave
  (block_on); altrimenti fan-out a CONSOLE (serial+fb).
- Router (`exec_worker_task`) passa `Some(slot.term_pts)`; i boot-check passano
  `None` (output su seriale per il grep).

Verificato in QEMU: `wtecho ROUTER-OK` (cwasm) stampa via pts0 → console → seriale
(`ROUTER-OK`), nessuna regressione su `echo`/boot.

## Perché
Allinea i `.cwasm` ai tool wasmi (output al terminale/SSH/pipeline, non bypass).
stdin resta EOF: il read bloccante su PTY richiede epoch/async (fibre Wasmtime),
spike separato non necessario per la GUI (che può fare gfx_poll_event poll-based).

## File toccati
- kernel/src/wasm/wt/{state.rs,wasi.rs,mod.rs}
- kernel/src/executor/mod.rs
