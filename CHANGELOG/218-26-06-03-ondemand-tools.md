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

NOTA (risolto): run-m2b2-test ora PASSA (TEST_PASS_M2B2) dopo il fix LFN-read nel
driver FAT32 del kernel (`kernel/src/vfs/fat32.rs`, commit separato). In origine
falliva perché il driver scriveva le entry LFN ma NON le rileggeva in lookup
(`read_dir_entries` saltava `ATTR_LFN`, `lookup` confrontava solo il nome 8.3): i
`*.wasm` (estensione a 4 char) finivano sul nome corto lossy `UNAME~1.WAS` ≠
`uname.wasm` richiesto → NotFound → `uname: not found`. Ora `read_dir_entries`
ricostruisce il nome lungo dalle run `ATTR_LFN` (per ordinale) e lo espone come
`DirEntry.name`, così l'exec on-demand risolve `/mnt/bin/uname.wasm` e `uname -a`
stampa `wasm-userland`. I file con solo nome corto (es. `/mnt/hello.txt`) restano
invariati → run-test resta verde.
