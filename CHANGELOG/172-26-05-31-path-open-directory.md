# 172 — WASI fd_readdir Task 2: OpenDir + path_open(O_DIRECTORY)

**Data:** 2026-05-31

## Cosa
`path_open` con flag `O_DIRECTORY` ora apre davvero le directory,
restituendo un `FdEntry::Dir` (introdotto in [[171-26-05-31-fdentry-dir]]).

- `kernel/src/wasm/suspend.rs`: nuovo `SuspendReason::OpenDir { path,
  opened_fd_ptr }`.
- `kernel/src/wasm/host/path.rs`: `path_open`, quando `oflags &
  OFLAGS_DIRECTORY != 0`, non passa più per il flusso file (che dava
  IsDirectory → errore); trappa con `OpenDir` sul path già risolto
  via `resolve_cwd`.
- `kernel/src/wasm/fiber.rs`: handler `OpenDir` — `vfs::stat(&path)`;
  se è una directory alloca uno slot fd libero (skip 0/1/2, come
  `PathOpen`) con `FdEntry::Dir(path)` e scrive l'indice; se esiste ma
  non è dir → `54` (ENOTDIR); se non esiste → `44` (ENOENT).

## Perché
`fdopendir`/`opendir` di wasi-libc fanno `path_open(O_DIRECTORY)` per
ottenere un fd su cui poi chiamare `fd_readdir`. Prima questo fd non
esisteva. Con `fd_fdstat_get` che già riporta DIRECTORY per i Dir fd
(Task 1), la catena `opendir → fdstat check → fd_readdir` ora ha il
suo fd valido. Manca solo `fd_readdir` (Task 3).

## Test
`make build` pulito. `path_open(O_DIRECTORY)` su `/bin` ora ritorna un
fd valido invece di un errore; verificabile end-to-end solo dopo Task 3
(serve `fd_readdir` per consumare il fd).

## File toccati
- kernel/src/wasm/suspend.rs
- kernel/src/wasm/host/path.rs
- kernel/src/wasm/fiber.rs
