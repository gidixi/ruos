# 171 — WASI fd_readdir Task 1: FdEntry::Dir + match arms

**Data:** 2026-05-31

## Cosa
Primo passo per esportare `fd_readdir` (vedi spec
`docs/superpowers/specs/2026-05-31-rust-fd-readdir-design.md`): modellare
una directory aperta come fd di prima classe.

- `kernel/src/wasm/state.rs`: aggiunta variante `FdEntry::Dir(String)`
  che porta il path assoluto risolto. Nessun handle VFS da rilasciare —
  l'enumerazione avviene per-chiamata in `fd_readdir`.
- `kernel/src/wasm/host/fd.rs`:
  - `fd_read` / `fd_write` su un fd directory → `21` (EISDIR).
  - `fd_close` su Dir → libera solo lo slot (nessun VfsClose).
  - `fd_filestat_get` su Dir → filetype `DIRECTORY` (3), size 0.
  - `fd_fdstat_get` su Dir → `fs_filetype = 3` (DIRECTORY). **Load-bearing**:
    `fdopendir` di wasi-libc verifica questo *prima* di chiamare
    `fd_readdir`; senza, `read_dir` fallirebbe con ENOTDIR a monte.

## Perché
`FdEntry` aveva solo `StdoutConsole`/`Vfs`/`Socket`: nessun modo di
rappresentare una directory aperta. Senza questa variante, i task 2-3
(`path_open(O_DIRECTORY)` + `fd_readdir`) non hanno un fd su cui
operare. `ruos.readdir` (host fn custom 12-byte) resta intatto.

## Test
`make build` pulito (release). Nessun cambiamento di comportamento
osservabile ancora — la variante è inerte finché `path_open` non la
produce (Task 2).

## File toccati
- kernel/src/wasm/state.rs
- kernel/src/wasm/host/fd.rs
