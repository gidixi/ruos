# 111 — fd_fdstat_get returns real filetype + full rights

**Data:** 2026-05-29

## Cosa

`fd_fdstat_get` ritornava 24 byte zero. Conseguenza: `fs_filetype = 0`
(UNKNOWN) + `fs_rights_base = 0` → wasi-libc trattava FD come "no
permissions" e silenziosamente rifiutava read/write su file aperti
via path_open.

Fix:
1. **FD 3 (preopen "/") → 3 DIRECTORY**. Era il caso più importante:
   senza, wasi-libc non poteva risolvere path relativi.
2. **FdEntry::Vfs(fd)** → query `vfs::stat_fd` per kind reale (Reg=4,
   Dir=3, Device=2).
3. **FdEntry::StdoutConsole** → 2 (CHARACTER_DEVICE).
4. **FdEntry::Socket(_)** → 7 (SOCKET_STREAM).
5. **Rights base + inheriting** → `u64::MAX` (grant all). Kernel non
   enforce ACL.

stat_fd usa `vfs::block_on` (noop_waker) — tmpfs stat single-poll OK.

## Perché

Sblocca shell.wasm reading `/etc/init.sh` correttamente. Era già
funzionante per shell ma altri tool (mkdir, ls) potevano fallire
silenziosamente.

## File toccati

- kernel/src/wasm/host/fd.rs
- CHANGELOG/111-26-05-29-fd-fdstat-real-filetype.md (nuovo)

## Cp ancora hang

cp.wasm hangs in `Fiber::new` (wasmi instantiate). Separato. Vedi
`docs/followups/cp-wasm-instantiate-hang.md`.
