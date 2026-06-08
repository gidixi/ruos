# 349 — Live-CD: skip porta CD nel loop SATA + asserzioni run-test

**Data:** 2026-06-08

## Cosa
- **Fix VirtualBox**: quando il CD-ROM ATAPI sta sul boot HBA (VBox: AHCI singolo,
  CD su port 0), il loop SATA `/mnt` rifaceva `bringup` sulla stessa porta già
  posseduta dal mount ISO9660 `/bin`, riprogrammandone PxCLB/PxFB e corrompendo le
  letture del CD (→ `shell.wasm NotFound`). Ora `acquire_atapi_port` registra
  l'indice della porta CD del boot HBA (`ahci::boot_cd_port()`) e il loop SATA la
  salta; salta anche qualsiasi porta ATAPI (non è un disco FAT).
- **Test**: aggiunte asserzioni a `make run-test` — `ahci port N atapi sectors=`
  e `/bin overlaid from ISO9660`.

## Perché
In QEMU q35 il CD è su un controller AHCI diverso dal disco, quindi il bug non
emergeva; in VBox (AHCI singolo, CD e mount sulla stessa porta) sì. Verificato:
VBox boota fino al desktop col CD su controller SATA; QEMU `make run-test` resta
verde (/mnt FAT montato, /bin dal CD).

## File toccati
- kernel/src/ahci/mod.rs
- kernel/src/boot/phases/storage.rs
- Makefile
- CHANGELOG/349-26-06-08-livecd-cd-port-skip-and-asserts.md
