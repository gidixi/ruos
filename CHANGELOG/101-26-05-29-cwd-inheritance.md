# 101 — Per-fiber CWD + inheritance + relative path resolution

**Data:** 2026-05-29

## Cosa

User bug: `cd /bin` poi `ls` mostrava ancora `/`. Causa: shell.wasm
teneva CWD localmente (`static Mutex<String>`), ma `ls.wasm` come
processo separato non lo ereditava → defaultava a `/`.

Fix POSIX-style: ogni Fiber ha CWD in `RuntimeState`, ereditato via
`ruos_exec`, e tutti i path relativi sono risolti vs CWD lato kernel.

### Kernel-side

- `RuntimeState.cwd: String` (default `"/"`).
- `Fiber::set_cwd(cwd)` method.
- `SuspendReason::Exec { ..., cwd: String }` carries parent's CWD.
- `exec_queue::ExecSlot.cwd`, `post_and_wait(path, argv, cwd)`,
  `ExecFuture.cwd`.
- `exec_worker_task` calls `child.set_cwd(slot.cwd)` after
  `set_args` and before `run().await`.
- New host fn `ruos::chdir(path_ptr, path_len) -> errno` writes
  caller's `RuntimeState.cwd`.
- `path_open` resolves relative paths via `proc::resolve_cwd(&caller.
  data().cwd, path)`.
- `ruos_readdir` same.
- `resolve_cwd`: handles `.`/`..`/absolute override + dedup slashes.

### User-side

- `user/shell/src/main.rs::builtin_cd`: chiama `ruos::chdir` (kernel
  CWD) PRIMA di aggiornare la copia locale `CWD` (per prompt + history).
- `user/ls/src/main.rs`: default path `"."` invece di `"/"` quando
  nessun arg.

## Perché

POSIX semantics: child processes inherit parent's working directory.
Step 11/12 omitting this broke `cd foo; ls` workflow.

Resolution kernel-side: tutti i wasm guest (ls, cat, echo, future bash)
diventano CWD-aware senza modifiche guest.

## File toccati

- kernel/src/wasm/state.rs (+cwd field)
- kernel/src/wasm/fiber.rs (+set_cwd; Exec arm cwd)
- kernel/src/wasm/suspend.rs (+cwd in Exec)
- kernel/src/wasm/host/proc.rs (+ruos_chdir +resolve_cwd; ruos_exec
  captures + ruos_readdir resolves)
- kernel/src/wasm/host/path.rs (path_open resolves)
- kernel/src/wasm/exec_queue.rs (cwd in ExecSlot+ExecFuture)
- kernel/src/executor/mod.rs (set_cwd on child)
- user/shell/src/main.rs (cd calls chdir)
- user/ls/src/main.rs (default ".")
- user-bin/ls.wasm + user-bin/shell.wasm (rebuilt)
- CHANGELOG/101-26-05-29-cwd-inheritance.md (nuovo)
