# 62 — wasmi runtime up + lifecycle + fd_write console (Step 10 Task 2)

**Data:** 2026-05-28

## Cosa

- Aggiunte deps `wasmi = "1.0.9"` (no_std, prefer-btree-collections) +
  `embassy-futures = "0.1"` in `kernel/Cargo.toml`.
  Nota: il piano citava "0.36" che non esiste; la versione stabile no_std
  è 1.0.9 con API Engine/Module/Store/Linker/Caller analoga ma diversa.
- Nuovo `kernel/src/wasm/state.rs`: `RuntimeState` con tabella FD (16 slot);
  FD 1/2 mappati a `FdEntry::StdoutConsole`.
- Nuovo `kernel/src/wasm/host/lifecycle.rs`: `args_sizes_get`, `args_get`,
  `environ_sizes_get`, `environ_get`, `proc_exit` (5 host fns).
  `proc_exit` usa `Error::i32_exit(code)` (built-in wasmi 1.x) invece di
  un `WasiTrap` custom. Exit code recuperato con `e.kind().as_i32_exit_status()`.
- Nuovo `kernel/src/wasm/host/fd.rs`: `fd_write` (verso console tramite
  `CONSOLE.lock().write_str()`), stubs `fd_read`/`fd_close`/`fd_seek`/
  `fd_fdstat_get`/`fd_prestat_get`/`fd_prestat_dir_name`.
- Nuovo `kernel/src/wasm/host/mod.rs`: aggrega `lifecycle` + `fd`.
- Nuovo `kernel/src/wasm/mod.rs`: `Runtime` wrapper su wasmi
  `Engine`/`Module`/`Store`/`Instance`. `run()` chiama `_start` e
  cattura i32_exit. `run_at(path)` legge da VFS, istanzia, esegue.
- `kernel/src/main.rs`: aggiunto `mod wasm;`.
- `kernel/src/executor/mod.rs`: spawna `wasm_task("/init.wasm")` come 3° task.
- Nuovo `user/` workspace con crate `init` (welcome banner ANSI).
- `Makefile`: target `user-wasm` builda `wasm32-wasip1`, copia in
  `user-bin/init.wasm`; `iso` dipende da `$(USER_WASM)`.
- HELLO sentinel → `ruos: init.wasm exited cleanly`.

## Adattamenti API wasmi 1.x vs piano

- `linker.instantiate_and_start()` (non `pre.start()` separato).
- `Memory::read/write(ctx, offset, buf)` accetta `impl AsContext/AsContextMut`;
  `Caller` implementa entrambi.
- `Error::i32_exit(code)` built-in (non WasiTrap custom HostError).
- `error.kind().as_i32_exit_status()` per recuperare il codice di uscita.
- `wasmi::errors::Error` → `wasmi::Error` (re-export diretto).

## Perché

Secondo task dello Step 10. Materializza il "wasm runs" end-to-end:
binario Rust reale wasm32-wasip1, caricato via Limine module/VFS tmpfs,
instanziato da wasmi, stampa welcome ANSI, termina clean.
`make run-test` → `TEST_PASS` confermato.

## File toccati

- kernel/Cargo.toml
- kernel/Cargo.lock
- kernel/src/main.rs
- kernel/src/executor/mod.rs
- kernel/src/wasm/mod.rs (nuovo)
- kernel/src/wasm/state.rs (nuovo)
- kernel/src/wasm/host/mod.rs (nuovo)
- kernel/src/wasm/host/lifecycle.rs (nuovo)
- kernel/src/wasm/host/fd.rs (nuovo)
- Makefile
- user/Cargo.toml (nuovo)
- user/init/Cargo.toml (nuovo)
- user/init/src/main.rs (nuovo)
- user-bin/init.wasm (rigenerato, 41832 bytes)
- CHANGELOG/62-26-05-28-wasix-wasmi-up.md (nuovo)
