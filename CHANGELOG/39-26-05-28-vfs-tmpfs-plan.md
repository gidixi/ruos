# 39 — Piano implementazione: VFS + tmpfs + devices (Step 7)

**Data:** 2026-05-28

## Cosa

Scritto il piano dello Step 7 in
`docs/superpowers/plans/2026-05-28-rust-vfs-tmpfs.md`. Tre task:

1. **Core types** — `vfs/{error,path,file,fs,fd,block_on,mod}.rs` con trait
   `FileSystem`/`File` (async AFIT), `FsImpl`/`FileImpl` enum placeholder,
   `OpenFlags` (bitflags), `Whence`, `Fd`, `FDS` table, `block_on` noop_waker,
   `path::split`. Build green, nessun runtime change.
2. **tmpfs + devices + init** — `vfs/{tmpfs,devices}.rs`, riempi
   `FileImpl`/`FsImpl` con variant reali, `vfs::init()` mounta `/` su tmpfs e
   crea `/dev/{console,null,zero}` + `/tmp`. kmain logga
   `ruos: vfs init ok mounts=1`.
3. **API dispatch + smoke test** — `vfs::open/close/read/write/seek` su
   `MOUNTS` + `FDS`. Smoke test al boot (open `/dev/null` write; create
   `/tmp/x` write+seek+read). Logga `ruos: vfs smoke ok n=3 buf=[abc]`.

`TEST_PASS` preservato a ogni checkpoint.

## Perché

Tradurre lo spec Step 7 in passi eseguibili e verificabili.

## File toccati

- docs/superpowers/plans/2026-05-28-rust-vfs-tmpfs.md
- CHANGELOG/39-26-05-28-vfs-tmpfs-plan.md
