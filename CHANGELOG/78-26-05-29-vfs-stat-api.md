# 78 — stat API + tmpfs impl

**Data:** 2026-05-29

## Cosa

Aggiunto al `FileSystem` trait:
```rust
async fn stat(&self, path: &[&str]) -> Result<VfsStat, VfsError>;
```

Tipo nuovo in `vfs/fs.rs`:
- `pub struct VfsStat { kind: VfsKind, size: u64 }`

tmpfs impl: walk al path, lock inode, restituisce kind + size
(`content.len()` per Reg, 0 per Dir/Device).

API pubblica `vfs::stat(path: &str) -> Result<VfsStat, _>`. Re-export
`VfsStat` da `vfs::mod`.

## Perché

Pre-Step 11. `ls.wasm` chiamerà readdir poi stat per entry per
mostrare type + size. `cat.wasm` chiamerà stat per allocare buffer
read della dimensione esatta.

`stat` separato da `open` per evitare allocazione FD per query
metadati. Pattern Unix POSIX standard (`stat(2)` non apre).

## File toccati

- kernel/src/vfs/fs.rs
- kernel/src/vfs/tmpfs.rs
- kernel/src/vfs/mod.rs
- CHANGELOG/78-26-05-29-vfs-stat-api.md (nuovo)
