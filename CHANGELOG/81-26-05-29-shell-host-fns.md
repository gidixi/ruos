# 81 — ruos_exec + ruos_readdir host fns + real args_* lifecycle

**Data:** 2026-05-29

## Cosa
- Added `Exec` and `ReadDir` variants to `SuspendReason` enum (owned types, Clone-friendly).
- Created `kernel/src/wasm/host/proc.rs`: custom `ruos` namespace host functions `ruos_exec` and `ruos_readdir` that suspend via `SuspendReason`, plus `decode_argv` helper for the packed argv blob format.
- Registered `proc::link` in `kernel/src/wasm/host/mod.rs`.
- Added `Fiber::set_args` method to inject argv into `RuntimeState`.
- Added `Exec` dispatch arm in `Fiber::dispatch`: reads child wasm bytes via `read_all`, instantiates a child `Fiber`, calls `child.set_args(argv)`, runs via `Box::pin(child.run()).await` (box required for recursive async), writes exit code.
- Added `ReadDir` dispatch arm in `Fiber::dispatch`: calls `vfs::readdir`, serializes entries (kind byte, padding, name_len u16 LE, size u64 LE, name bytes), writes to wasm buffer.
- Promoted `read_all` to `pub(crate)` in `kernel/src/wasm/mod.rs` so `fiber.rs` can call it.
- Called `fb.set_args(vec![path.as_bytes().to_vec()])` in `run_at` after instantiation.
- Replaced stub `args_get` in `lifecycle.rs` with real implementation: writes argv pointers + null-terminated strings.
- Updated `user/init/src/main.rs` to print `init.wasm: argv0=<arg0>` as first output.
- Updated Makefile `HELLO` sentinel to `init.wasm: argv0=/init.wasm`.

## Perché
Step 11 Task 1: expose the process-launch (`ruos_exec`) and directory-listing (`ruos_readdir`) host function surface needed by the upcoming shell.wasm. Also wire up real `args_*` WASI lifecycle functions so any wasm app can read its argv.

## File toccati
- kernel/src/wasm/suspend.rs
- kernel/src/wasm/host/proc.rs (new)
- kernel/src/wasm/host/mod.rs
- kernel/src/wasm/fiber.rs
- kernel/src/wasm/mod.rs
- kernel/src/wasm/host/lifecycle.rs
- user/init/src/main.rs
- user-bin/init.wasm
- Makefile
