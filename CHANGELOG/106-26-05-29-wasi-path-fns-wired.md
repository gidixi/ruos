# 106 — WASI path_* host fns wired

**Data:** 2026-05-29

## Cosa
Le stub ENOSYS in `kernel/src/wasm/host/path.rs` (path_unlink_file,
path_create_directory, path_remove_directory, path_filestat_get) ora
trappano in `SuspendReason::PathUnlink/PathMkdir/PathRmdir/PathFilestat`
e vengono completate da `Fiber::dispatch` chiamando le nuove API VFS.
Aggiunta inoltre `path_rename` (`SuspendReason::PathRename`) per
abilitare `std::fs::rename` da wasm32-wasip1.

Aggiunte cinque varianti a `SuspendReason` (PathUnlink/Mkdir/Rmdir/
Filestat/Rename). Tutte risolvono il path relativo contro `caller.cwd`
prima di chiamare il VFS (stessa pipeline di `path_open`).

`path_filestat_get` scrive la layout `wasi_filestat_t` (64 byte) — solo
filetype + size; gli altri campi (dev/ino/nlink/atim/mtim/ctim) restano
0, consistente con `fd_filestat_get`.

Mapping errno: VfsError → wasi errno (ENOENT=44, EISDIR=31, EEXIST=20,
ENOTDIR=54, ENOTEMPTY=55, EINVAL=28, fallback EBADF=8).

## Perché
Sblocca `std::fs::create_dir(_all)`, `remove_dir`, `remove_file`,
`rename`, `metadata` dai .wasm userspace. Risolve il TODO
"path_filestat_get stub forces cat into slow fallback" anche per
path-based (oltre al già wired `fd_filestat_get`).

## File toccati
- kernel/src/wasm/host/path.rs
- kernel/src/wasm/suspend.rs
- kernel/src/wasm/fiber.rs
