# 409 — modules: prefissi /archive/ + /rescue/ e accessor

**Data:** 2026-06-10

## Cosa
Aggiunti due nuovi prefissi cmdline Limine a `kernel/src/modules.rs`:
- `ARCHIVE_PREFIX = "/archive/"` — archivio /bin compresso (`bin.bgz`); skippato
  dal tmpfs-mount come `/payload/`; recuperato via nuova fn `archive(name)`.
- `RESCUE_PREFIX = "/rescue/"` — set rescue (shell + tool minimi); skippato dal
  tmpfs-mount; enumerato via nuova fn `rescue_all()`.

`mount_all()` aggiornato a skippare i due nuovi prefissi (condizione `||` sulla
variabile `payloads`). Aggiunte `pub fn archive()` e `pub fn rescue_all()` in coda
al file, che seguono lo stesso pattern `transmute`/SAFETY di `payload()` / `all()`.

## Perché
Task 4 del piano bin-pack/livecd: il kernel deve ignorare questi moduli durante il
mount tmpfs e offrire accessor tipizzati per recuperarli dall'HHDM — usati nelle
fasi successive (`unpack_bin`, fallback rescue).

## File toccati
- kernel/src/modules.rs
