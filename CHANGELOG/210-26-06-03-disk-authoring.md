# 210 — M2a: disk authoring (GPT write + FAT32 mkfs + mkdir + mkdisk)

**Data:** 2026-06-03

## Cosa
ruos ora sa **creare un disco da zero** (write-side dell'installer su SSD, M2a):
scrive una GPT (MBR protettivo + header primario/backup + CRC32), formatta FAT32
(mkfs, geometria fatgen103) e crea un albero di directory (/EFI/BOOT sull'ESP).
Esposto come tool `mkdisk` (host fn `ruos_mkdisk`) che autora il primo disco SATA.
M1 (lettura GPT) montava già la partizione dati; ora la generiamo noi.

- kernel/src/crc32.rs (nuovo): CRC-32 riflesso (IEEE 802.3 / GPT).
- kernel/src/gpt.rs: write_layout (GPT completa + CRC); + validazione CRC in parse.
- kernel/src/vfs/fat32.rs: format (mkfs.fat32), mkdir + estensione catena dir,
  FatWriter + create_dirs (path-write sincrono per l'authoring).
- kernel/src/blockdev.rs: PartBorrow (vista di partizione a prestito).
- kernel/src/disk.rs (nuovo): author (GPT+format+/EFI/BOOT).
- kernel/src/wasm/host/proc.rs: host fn ruos_mkdisk; user/mkdisk (tool wasm).

## Perché
Prerequisito dell'installer self-hosted su SSD (M2b: copia kernel+BOOTX64.EFI
sull'ESP, comando install, boot da SSD). M2a è la capacità "creare il disco"; è
testabile da sola (autora in QEMU, verifica con sgdisk/fsck/mtools + round-trip M1).

## File toccati
- kernel/src/crc32.rs (nuovo), gpt.rs, vfs/fat32.rs, blockdev.rs, disk.rs (nuovo),
  main.rs, wasm/host/proc.rs, user/mkdisk/ (nuovo), Makefile, limine.conf,
  tests/m2a-test.sh (nuovo), user-bin/m2a-init.sh + m2a-rt-init.sh (nuovi)

## Verifica
make run-m2a-test → TEST_PASS_M2A (autora un disco GPT+FAT32 vuoto in QEMU,
verificato con sgdisk -v + fsck.fat + mtools /EFI/BOOT, + round-trip: M1 rimonta
la partizione dati e il marker persiste). make run-gpt-test → TEST_PASS_GPT,
make run-test → TEST_PASS (nessuna regressione).
