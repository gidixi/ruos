# 317 — C2b: route .cwasm shell-exec to a ComputeApp core

**Data:** 2026-06-06

## Cosa
Implementato C2b: un'app `.cwasm` eseguita dalla shell gira su un core
`ComputeApp` (non sul BSP), così l'executor del BSP rimane libero per I/O
(net/usb/ssh) durante l'esecuzione dell'app. Single-slot ancora (il parallelismo
è C2c).

Meccanismo:
- `AppReply`/`AppReplyFuture` (statico single-slot) per la notifica cross-core di
  completamento: il task AP chiama `APP_REPLY.complete(code)` → sveglia il waiter
  BSP via il pender cross-core (Step 2).
- Task `run_app_on_core(bytes: Box<[u8]>, argv, pts)` (embassy, `pool_size=1`):
  gira su qualunque core venga spawnato (sempre ComputeApp), esegue `run_cwasm`,
  logga `exec-ap ran_on=core{N} code={C}`, completa `APP_REPLY`.
- `first_compute_app_core()` in `cpu/mod.rs`: primo core online con ruolo
  `ComputeApp`, o `None` (fallback inline su BSP per sistemi 1-2 core).
- `exec_worker_task` (BSP): dopo `proc::register`, controlla `first_compute_app_core()`.
  Se `Some(core)`: arma APP_REPLY, `bytes.into_boxed_slice()` → `spawn_on(core,
  run_app_on_core(...))`, poi `.await AppReplyFuture` (BSP cede il controllo).
  Se `None`: esegue inline con `&bytes` come prima.
- Fix pre-esistente: pattern grep USB keyboard in `run-test` (`"usb  keyboard ready"`
  → `"usb.*keyboard ready"`) perché il log ora include `"hid boot"`.
- Gate: `user-bin/exec-ap-init.sh` + target `run-exec-ap-test` nel Makefile.

## Perché
BSP executor libero durante l'esecuzione di app `.cwasm` pesanti → SSH/net/usb
restano responsivi. Fondamenta per C2c (parallelismo: più app contemporaneamente
su core diversi).

## File toccati
- `kernel/src/executor/mod.rs` — AppReply/AppReplyFuture, run_app_on_core task, routing in exec_worker_task
- `kernel/src/cpu/mod.rs` — first_compute_app_core()
- `user-bin/exec-ap-init.sh` — script gate C2b
- `Makefile` — target run-exec-ap-test + fix grep USB KBD
- `CHANGELOG/317-26-06-06-smp-c2b-route-cwasm-exec-to-compute-core.md`
