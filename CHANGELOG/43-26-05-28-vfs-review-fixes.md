# 43 — Review fix Step 7 (VFS + tmpfs)

**Data:** 2026-05-28

## Cosa

Correzioni dalle code review dello Step 7:

- **Task 2:** overflow guards in `TmpfsFile` (`checked_add` per `write` end e
  `seek` base+off), `vfs::init` idempotente (`AlreadyExists` se MOUNTS già
  popolato), nuova variant `VfsError::Invalid`, commento single-poll
  invariant sulla CREATE-path di `Tmpfs::open`, capture diretto dell'`Arc`
  appena inserito al posto di re-lookup.
- **Task 3 / finale:**
  - `vfs::write` e `vfs::seek` ora hanno commento esplicito sul caveat
    "FDS lock held across await" (era solo su `read`).
  - `Tmpfs::unlink` rifiuta directories con `VfsError::IsDirectory`
    (impedirebbe altrimenti drop ricorsivo silenzioso di /dev e contenuti).
  - `OpenFlags` doc-commentate: enforcement (READ/WRITE/TRUNCATE access
    checks) rimandato a Step 10 WASI; per Step 7 solo `CREATE` è onorato.

## Perché

Chiudere i rilievi delle review prima del merge di Step 7. TEST_PASS
preservato (`ruos: ticks=10`, smoke `n=3 buf=[abc]`).

## File toccati

- kernel/src/vfs/error.rs (Task 2 fix — variant Invalid)
- kernel/src/vfs/tmpfs.rs (Task 2 + finale: overflow guards, unlink dir guard, CREATE comment)
- kernel/src/vfs/mod.rs (Task 2 fix init idempotente; Task 3 fix commento write/seek)
- kernel/src/vfs/file.rs (Task 3 finale: doc OpenFlags enforcement)
- CHANGELOG/43-26-05-28-vfs-review-fixes.md
