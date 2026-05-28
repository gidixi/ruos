# 38 — Spec design: VFS minimale + tmpfs + device files (Step 7)

**Data:** 2026-05-28

## Cosa

Scritta la spec dello Step 7 in
`docs/superpowers/specs/2026-05-28-rust-vfs-tmpfs-design.md`. API VFS **async**
(AFIT Rust 1.75+) con dispatch **enum** (no Box, no async-trait crate):

- `FsImpl { Tmpfs(...) }` + `FileImpl { Tmp/Console/Null/Zero }`.
- Trait `FileSystem` (open/create/unlink) + `File` (read/write/seek).
- FD table globale `Vec<Option<FdEntry>>` numerica (compat WASI).
- Mini `block_on` con noop_waker (chiamare async dal kmain prima dell'executor
  embassy di Step 9; tmpfs+devices risolvono single-poll).
- tmpfs in-RAM: `Arc<Mutex<TmpInode>>` con tree (Dir/Reg).
- Devices al boot: `/dev/console` (write→SERIAL), `/dev/null`, `/dev/zero`.
  `/dev/random` rimandato a Step 14 (CSPRNG RDRAND).
- Smoke test al boot: open `/dev/null` write; create `/tmp/x` + write + seek
  + read back; log `ruos: vfs smoke ok n=3 buf=[abc]`.

Layout `kernel/src/vfs/{mod,error,path,fs,file,fd,tmpfs,devices,block_on}.rs`.
Decomposizione 3 task; `TEST_PASS` preservato a ogni checkpoint.

## Perché

Step 7 della roadmap WASM-first: prerequisito per Step 10 (WASI host
functions `fd_*`/`path_*`) e Step 11 (shell che cerca `.wasm` per path).

## File toccati

- docs/superpowers/specs/2026-05-28-rust-vfs-tmpfs-design.md
- CHANGELOG/38-26-05-28-vfs-tmpfs-spec.md
