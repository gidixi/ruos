# 211 — M2b-1: copia payload di boot su ESP (write_file FAT + LFN)

**Data:** 2026-06-03

## Cosa
ruos ora copia il proprio albero di boot su un ESP autorato (M2a), così l'SSD
parte da solo: kernel + BOOTX64.EFI + limine.conf (spediti come moduli Limine) +
tutti i moduli .wasm/init, ai path che limine.conf si aspetta. Aggiunto al
FatWriter la scrittura file FAT32 con **nomi lunghi (LFN)** (tutti i .wasm e
limine.conf non sono 8.3) e allocazione O(n) (hint next-free + scritture FAT/dati
in blocco) per reggere il kernel da ~20 MB. Esposto come tool `mkboot` (autora +
copia). M2b-2: comando install + guardie disco + boot da SSD (OVMF).

## File toccati
- kernel/src/vfs/fat32.rs (write_file + LFN + alloc O(n)), kernel/src/modules.rs
  (payload/all + skip /payload/), kernel/src/disk.rs (copy_boot_payload),
  kernel/src/wasm/host/proc.rs (ruos_mkboot), limine.conf (3 moduli payload +
  mkboot), user/mkboot/ (nuovo), Makefile, tests/m2b1-test.sh + user-bin/m2b1-init.sh

## Verifica
make run-m2b1-test → TEST_PASS_M2B1 (autora+copia in QEMU; ESP estratto:
sgdisk -v pulito, fsck.fat pulito, mdir mostra BOOTX64.EFI/kernel/limine.conf +
nomi lunghi .wasm, kernel da 20 MB byte-identico all'ISO). run-m2a-test,
run-gpt-test, run-test → tutti verdi.
