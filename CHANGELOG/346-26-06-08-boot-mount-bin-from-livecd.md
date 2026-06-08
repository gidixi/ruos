# 346 — Boot: mount /bin dal live-CD (ISO9660/ATAPI)

**Data:** 2026-06-08

## Cosa
La fase storage monta `/bin` dal CD-ROM ATAPI (ISO9660) se presente; fallback a `modules::mount_all()` (moduli Limine) se non c'è CD. La fase fs non monta più i bin.

## Perché
Boot più elegante: Limine carica solo il kernel; i bin si leggono on-demand dal CD invece di essere pre-caricati in RAM e ricopiati in tmpfs.

## File toccati
- kernel/src/boot/phases/storage.rs
- kernel/src/boot/phases/fs.rs
- CHANGELOG/346-26-06-08-boot-mount-bin-from-livecd.md
