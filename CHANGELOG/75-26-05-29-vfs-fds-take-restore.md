# 75 — FDS take-and-restore in vfs::read/write/seek

**Data:** 2026-05-29

## Cosa

Riscritto `vfs::read`/`write`/`seek` con pattern take-and-restore via
helper `with_fd_take`:

1. Lock `FDS` brevemente.
2. `take()` la `FdEntry` fuori dallo slot.
3. Rilascia `FDS` lock.
4. Esegui `entry.file.read/write/seek.await` (può suspendere).
5. Re-lock `FDS`. Se slot ancora None → restore. Se slot riassegnato da
   open() concorrente → drop la nostra entry (close-during-IO semantics).

## Perché

Pattern precedente teneva `FDS.lock()` attraverso l'await. Safe oggi
con tmpfs single-poll, ma:

- Bloccato gli altri fiber che volevano fare VFS I/O su FD diverso.
- Sarebbe stato deadlock con un FS suspending (es. disk FS Step 14).
- Era documentato come "Revisit when Step 9 introduces a real executor".
  Step 10.5 lo ha fatto.

Pre-emptive cleanup. Ogni VFS Fd è owned da una sola fiber → no race
sulla restore window.

## File toccati

- kernel/src/vfs/mod.rs
- CHANGELOG/75-26-05-29-vfs-fds-take-restore.md (nuovo)
