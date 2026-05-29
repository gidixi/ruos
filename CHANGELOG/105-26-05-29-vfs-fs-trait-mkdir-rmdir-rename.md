# 105 — VFS FileSystem trait: mkdir / rmdir / rename

**Data:** 2026-05-29

## Cosa
Esteso `FileSystem` trait con `mkdir`, `rmdir`, `rename` async; aggiunte
implementazioni in `Tmpfs` (rmdir richiede dir vuota; rename gestisce
same-parent in-place e cross-parent con lock-ordering stabile sui due
inode parent per evitare deadlock). Aggiunti wrapper `vfs::mkdir`,
`vfs::rmdir`, `vfs::rename`, `vfs::unlink` (mancavano).

Rinominato `Tmpfs::mkdir` (sincrono, usato dal boot seeding) in
`Tmpfs::mkdir_sync` per non collidere col metodo async del trait — il
nome `mkdir` resta come metodo del trait.

## Perché
Sblocca cp/mv/rm/mkdir/rmdir userspace dopo aver wired le WASI host fns
corrispondenti (vedi entry 106). Senza queste API VFS, le WASI stub
`path_unlink_file` / `path_create_directory` / `path_remove_directory`
/ `path_rename` non hanno nulla da chiamare.

## File toccati
- kernel/src/vfs/fs.rs
- kernel/src/vfs/tmpfs.rs
- kernel/src/vfs/mod.rs
