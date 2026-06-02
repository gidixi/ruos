# 209 — GPT partition parse + mount partizione dati (boot da SSD)

**Data:** 2026-06-02

## Cosa
ruos ora parsa la GPT di un disco SATA e monta la partizione dati FAT32
(Microsoft Basic Data) come /mnt, persistente. Se il disco non ha GPT (FAT
grezza a LBA 0, come il disk.img QEMU) usa il fallback a LBA 0 di prima.
M1 del milestone installer su SSD (M2 = self-installer: GPT write + mkfs + copia).

- kernel/src/gpt.rs: parser GPT (header LBA1 + entries, type-GUID ESP/dati).
- kernel/src/blockdev.rs: PartitionDevice (wrapper offset base-LBA).
- kernel/src/vfs/fat32.rs: mount_from_blockdev (mount su qualsiasi BlockDevice).
- kernel/src/boot/phases/storage.rs: GPT-aware → monta la partizione dati;
  fallback LBA 0.

Questo commit (Task 5, finale): aggiunge il test end-to-end del percorso di
lettura M1. tests/gpt-test.sh costruisce un disco GPT da 64 MiB (ESP EF00 1 MiB +
Microsoft-Basic-Data 0700 con un file marker GPTHELLO.TXT), avvia QEMU con quel
disco come unico AHCI, e asserisce che ruos parsa la GPT + monta la partizione
dati come /mnt + legge il marker. Il target Makefile run-gpt-test ricostruisce
l'ISO con la smoke battery come init (come run-test) così la shell di boot fa il
`cat` di /mnt/GPTHELLO.TXT su seriale. user-bin/smoke.sh legge il marker
(silenzioso quando assente sul disco raw-FAT di run-test).

## Perché
Fondamenta per boot-da-SSD persistente (e prerequisito del self-installer M2:
per copiare file sull'ESP/dati creati serve montare partizioni a offset).

## File toccati
- kernel/src/gpt.rs (nuovo), blockdev.rs, vfs/fat32.rs, boot/phases/storage.rs,
  main.rs, tests/gpt-test.sh (nuovo), Makefile, user-bin/smoke.sh

## Verifica
make run-gpt-test → TEST_PASS_GPT (disco GPT: partizione dati montata + marker
letto). make run-test → TEST_PASS (fallback raw-FAT, nessuna regressione).

Evidenza seriale (run-gpt-test):
  storage gpt: data part lba=4096 sectors=126943 -> /mnt
  fat32   mnt mounted FAT
  gpt-persist-ok

Note sul test (non-banalità incontrate, tutte lato test-script):
- mkfs.vfat senza -F 32 su un volume da ~63 MiB sceglie FAT16; il driver ruos
  monta solo FAT32. La partizione dati ora è formattata con `-F 32`.
- mkfs.vfat -C rifiuta (exit 1) di creare su un file preesistente, lasciando in
  posto un FAT stantio. Il test ora fa `rm -f build/data.fat` prima di mkfs.
- La shell ruos non supporta l'operatore `||` (lo interpreta come pipe con
  segmento vuoto → "empty pipeline segment"), quindi la riga del marker in
  smoke.sh usa solo `cat ... 2>/dev/null` (la redirezione stderr funziona; il
  `|| true` è stato rimosso). Su disco raw-FAT il file è assente → output muto.
