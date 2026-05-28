# 41 — VFS tmpfs + devices + boot init (Step 7 Task 2)

**Data:** 2026-05-28

## Cosa
- `kernel/src/vfs/file.rs`: `FileImpl` placeholder rimpiazzato con
  `Tmp(TmpfsFile)`, `Console(ConsoleFile)`, `Null(NullFile)`, `Zero(ZeroFile)`;
  dispatch read/write/seek su tutte le variant.
- `kernel/src/vfs/fs.rs`: `FsImpl::Tmpfs(Tmpfs)`.
- `kernel/src/vfs/tmpfs.rs`: `Tmpfs` + `TmpInode` + `TmpfsFile`. Albero
  `Arc<Mutex<TmpInode>>`. Impl `FileSystem` (open con CREATE, create, unlink).
  Impl `File` per `TmpfsFile` (read/write/seek su Vec<u8>).
- `kernel/src/vfs/devices.rs`: `ConsoleFile` (write→SERIAL byte-per-byte,
  read=0, seek=NotPermitted), `NullFile`, `ZeroFile`.
- `kernel/src/vfs/mod.rs`: `MOUNTS` static + `mount(prefix, fs)` + `init()`
  che costruisce tmpfs, mkdir `/dev` + `/tmp`, crea `/dev/{console,null,zero}`,
  monta `/`. Ritorna numero di mount.
- `kmain`: dopo `sti`, chiama `vfs::init()` e logga
  `ruos: vfs init ok mounts=1`.

## Perché
Secondo pezzo dello Step 7: tmpfs + device files presenti e montati al boot.
API open/close/read/write/seek arriverà in Task 3.

## File toccati
- kernel/src/vfs/file.rs, fs.rs, tmpfs.rs (nuovo), devices.rs (nuovo), mod.rs
- kernel/src/main.rs
- CHANGELOG/41-26-05-28-vfs-tmpfs-devices.md
