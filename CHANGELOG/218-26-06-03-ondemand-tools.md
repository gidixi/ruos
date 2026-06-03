# 218 — Tool on-demand dalla partizione dati (initramfs slim su SSD)

**Data:** 2026-06-03

## Cosa
Il sistema INSTALLATO su SSD ora ha un ESP **slim**: solo il bootstrap (kernel +
shell + init + servizio rete/SSH), via una `limine.conf` ridotta. I ~50 comandi
(`ls`, `cat`, `grep`, `rtop`, …) stanno sulla **partizione dati** (`/mnt/bin`) e
la shell li carica **on-demand** dal FAT al primo uso (`resolve_path`: /bin →
/mnt/bin). Meno RAM/spazio al boot installato + tool fuori da `limine.conf`. La
**ISO live è invariata** (tutti i tool come moduli Limine — non c'è un fs da cui
leggere senza driver USB-storage).

- limine-ssd.conf (config slim, payload module); copy_boot_payload(dev,layout)
  splitta: bootstrap→ESP, tool→partizione dati; shell resolve_path /bin→/mnt/bin.

## File toccati
- limine-ssd.conf (nuovo), Makefile, limine.conf, kernel/src/disk.rs,
  kernel/src/wasm/host/proc.rs, user/shell/src/main.rs, tests/m2b1-test.sh,
  tests/m2b2-test.sh, user-bin/m2b2-init.sh

## Verifica
make run-m2b1-test → TEST_PASS_M2B1: ESP=bootstrap (BOOTX64.EFI + kernel +
limine.conf + shell.wasm + server.wasm; `ls.wasm` ASSENTE), tool su partizione
dati (`ls.wasm` + `readdirtest.wasm`, byte-identici), fsck pulito su entrambe.
Altri 3 gate verdi: run-m2a-test → TEST_PASS_M2A, run-gpt-test → TEST_PASS_GPT,
run-test → TEST_PASS.

NOTA (concern aperto): run-m2b2-test FALLISCE (TEST_FAIL_SSD_BOOT) — l'exec
on-demand NON funziona in-guest. Sul SSD avviato `/mnt` monta FAT, ma la shell
non risolve NESSUN tool da `/mnt/bin` (`echo: not found`, `uname: not found`).
Causa = bug del driver FAT32 del kernel (`kernel/src/vfs/fat32.rs`): scrive le
entry LFN ma NON le rilegge in lookup (`read_dir_entries` salta `ATTR_LFN`,
`lookup` confronta solo il nome 8.3). I `*.wasm` hanno estensione a 4 char →
nome corto lossy `UNAME~1.WAS` ≠ `uname.wasm` richiesto → NotFound. Il disco è
corretto (host mtools, che supporta LFN, legge `uname.wasm` dalla partizione
dati). Il fix è kernel-side (LFN read in fat32), fuori dallo scope di questo
task (solo test + init + changelog). Test corretti: m2b1 verde lo prova, e
m2b2 ora espone il bug invece di dare un falso PASS.
