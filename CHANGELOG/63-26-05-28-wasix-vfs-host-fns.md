# 63 — VFS-backed fd_* + path_* host fns (Step 10 Task 3)

**Data:** 2026-05-28

## Cosa

- `kernel/src/wasm/host/path.rs` (nuovo): `path_open` risolve un path
  wasm-side in un `vfs::Fd` via `embassy_futures::block_on`, alloca uno
  slot wasm-side nella tabella FD e lo restituisce al wasm.
- `kernel/src/wasm/host/fd.rs`: rimpiazzo degli stub Task 2 con
  implementazioni reali di `fd_read`, `fd_seek`, `fd_close`. `fd_write`
  ora dispatcha anche a `FdEntry::Vfs` tramite `vfs::write`.
  `fd_prestat_get`/`fd_prestat_dir_name` espongono un preopen `"/"` a
  fd=3 — necessario perché wasi-libc richiede almeno un preopen per
  abilitare le chiamate a `path_open` su path assoluti.
- Bridge sync→async: `embassy_futures::block_on` dentro le host fns
  (funziona perché tutte le future VFS correnti si risolvono in un
  singolo poll senza sospensione reale).
- `user/init/src/main.rs`: smoke apre `/wasm-smoke.bin` (CREATE|WRITE),
  scrive 10 byte, chiude, stampa `"init.wasm: vfs smoke ok"`.
- `Makefile`: `HELLO` aggiornato a `init.wasm: vfs smoke ok`.

## Perché

Terzo task dello Step 10 (WASIX bootstrap). Wiring completo I/O
wasm↔VFS tramite host fns WASIX reali; smoke test verifica il round-trip
open→write→close su tmpfs.

## File toccati

- kernel/src/wasm/host/path.rs (nuovo)
- kernel/src/wasm/host/fd.rs
- kernel/src/wasm/host/mod.rs
- user/init/src/main.rs
- user-bin/init.wasm (rigenerato)
- Makefile
- CHANGELOG/63-26-05-28-wasix-vfs-host-fns.md (questo file)
