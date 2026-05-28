# 40 — VFS core types + traits + path + block_on (Step 7 Task 1)

**Data:** 2026-05-28

## Cosa
- Aggiunta dep `bitflags = "2"`.
- Nuovo modulo `kernel/src/vfs/` con:
  - `error.rs` — `VfsError` enum + `Display`.
  - `path.rs` — `split(path)` (Vec<&str>, rifiuta '.', '..', componenti vuoti).
  - `file.rs` — trait `File` (async AFIT) + `FileImpl` placeholder + `OpenFlags`
    (`bitflags`) + `Whence` + `Fd = u32`.
  - `fs.rs` — trait `FileSystem` (async AFIT) + `FsImpl` placeholder.
  - `fd.rs` — `FDS: spin::Mutex<Vec<Option<FdEntry>>>` + `allocate/close`.
  - `block_on.rs` — noop_waker single-poll driver per chiamare async dal kmain
    finché Step 9 non porta `embassy-executor`.
  - `mod.rs` — re-exports.
- `main.rs`: `mod vfs;`.
- Nessun cambio runtime; placeholder variants + stati inerti finché Task 2/3
  riempiono.

## Perché
Primo pezzo dello Step 7: API surface + skeleton senza coinvolgere kmain.

## File toccati
- kernel/Cargo.toml, kernel/Cargo.lock
- kernel/src/vfs/* (nuovi)
- kernel/src/main.rs
- CHANGELOG/40-26-05-28-vfs-core-types.md
