# 82 — shell.wasm + ls/cat/echo + init.sh boot script

**Data:** 2026-05-29

## Cosa

- 4 new user crates (`wasm32-wasip1`): `shell`, `ls`, `cat`, `echo` in `user/`.
- `user-bin/init.sh` boot script: runs `echo hello from shell.wasm`, `pwd`, `ls /bin`, `echo init.sh end`.
- `limine.conf`: 5 new modules (`/etc/init.sh`, `/bin/shell.wasm`, `/bin/ls.wasm`, `/bin/cat.wasm`, `/bin/echo.wasm`).
- `Makefile`: 4 new per-crate build rules, `user-bin/init.sh` dependency, ISO staging with `build/iso_root/bin/` and `build/iso_root/etc/` subdirs.
- `vfs/mod.rs`: pre-create `/bin` and `/etc` directories in `vfs::init()`.
- `wasm/host/fd.rs`: add `fd_filestat_get` stub (type=REGULAR_FILE, size=0) required by `std::fs::read_to_string`.
- `wasm/exec_queue.rs`: new global single-slot exec queue + `ExecFuture`/`WaitForRequest` futures for decoupled child WASM execution.
- `executor/mod.rs`: add `exec_worker_task` that runs child WASMs on its own embassy stack (avoiding double-fault from recursive wasmi compilation); spawn `wasm_task("/bin/shell.wasm")` + `exec_worker_task`.
- `limine.conf`: `stack_size: 0x200000` (2 MiB) for future headroom.
- `Makefile`: `HELLO` sentinel updated to `shell: init.sh complete`.

## Perché

Step 11 Task 2: bring up shell userland. The shell reads `/etc/init.sh` at boot and
executes commands via the `ruos::exec` host function. Child WASMs (echo, ls) run in
a dedicated `exec_worker_task` to avoid stack overflow from wasmi eager compilation
happening recursively inside a fiber's stack frame.

## File toccati

- Makefile
- limine.conf
- user/Cargo.toml
- user/echo/Cargo.toml
- user/echo/src/main.rs
- user/cat/Cargo.toml
- user/cat/src/main.rs
- user/ls/Cargo.toml
- user/ls/src/main.rs
- user/shell/Cargo.toml
- user/shell/src/main.rs
- user-bin/init.sh
- kernel/src/vfs/mod.rs
- kernel/src/wasm/mod.rs
- kernel/src/wasm/exec_queue.rs
- kernel/src/wasm/host/fd.rs
- kernel/src/wasm/fiber.rs
- kernel/src/executor/mod.rs
