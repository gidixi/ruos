# 77 — readdir API + tmpfs impl

**Data:** 2026-05-29

## Cosa

Aggiunto al `FileSystem` trait:
```rust
async fn readdir(&self, path: &[&str]) -> Result<Vec<VfsDirent>, VfsError>;
```

Tipi nuovi in `vfs/fs.rs`:
- `pub enum VfsKind { Dir, Reg, Device }`
- `pub struct VfsDirent { name: String, kind: VfsKind }`

tmpfs impl: walk al path, lock dir inode, itera children, classifica
kind per ogni inode, restituisce `Vec<VfsDirent>`.

API pubblica `vfs::readdir(path: &str) -> Result<Vec<VfsDirent>, _>`.
Re-export `VfsDirent` + `VfsKind` da `vfs::mod`.

`MOUNTS` lock dropped prima del `.await` interno (anticipato il pattern
take-and-drop di Step 14 multi-mount).

## Perché

Pre-Step 11. `ls.wasm` chiamerà `ruos_readdir(path)` host fn che
trapsza `SuspendReason::ReadDir`, fiber dispatch invoca
`vfs::readdir(path).await`, formatta dirents in wasm memory.

Single-shot semantics (no cursor/cookie): Step 11 demo non ha cartelle
con migliaia di entries. Streaming readdir differito a Step 14+ (FS
disk).

## File toccati

- kernel/src/vfs/fs.rs
- kernel/src/vfs/tmpfs.rs
- kernel/src/vfs/mod.rs
- CHANGELOG/77-26-05-29-vfs-readdir-api.md (nuovo)
