# 212 — M2b-2: comando install + guardia /mnt + boot da SSD (capstone)

**Data:** 2026-06-03

## Cosa
Chiude il milestone installer-su-SSD: il comando `install` autora + copia
l'albero di boot sul primo disco SATA, con guardia di sicurezza (rifiuta se /mnt
è montato, così non cancella il sistema in esecuzione) e acquisizione porta
**senza HBA reset** (non orfanizza un /mnt vivo). Dopo `install` + reboot, l'SSD
avvia ruos da solo via UEFI (Limine → kernel) e M1 monta la sua partizione dati.

- kernel/src/ahci/mod.rs: acquire_port/sata_ports (reset-free), BOOT_HBA cache.
- kernel/src/vfs/mod.rs: is_mounted (guardia).
- kernel/src/wasm/host/proc.rs: ruos_install; mkdisk/mkboot ora reset-free.
- user/install/: tool. limine.conf/Makefile: install in /bin.

## Perché
È il traguardo: boot da chiavetta → install su SSD → boot da SSD persistente,
senza preparazione esterna. La guardia evita di cancellare il disco sbagliato +
previene il loop di re-install (al boot da SSD /mnt è montato → install rifiuta).

## File toccati
- kernel/src/ahci/mod.rs, kernel/src/vfs/mod.rs, kernel/src/wasm/host/proc.rs,
  user/install/ (nuovo), Makefile, limine.conf, tests/m2b2-test.sh (nuovo),
  user-bin/m2b2-init.sh (nuovo)

## Verifica
make run-m2b2-test → TEST_PASS_M2B2 (fase 1: install su disco vuoto in QEMU;
fase 2: boot UEFI/OVMF dal SSD senza cdrom → "ruos boot OK" + "mnt mounted FAT").
run-test/run-gpt-test/run-m2a-test verdi (run-m2b1-test verde al merge; lento su 9p).
