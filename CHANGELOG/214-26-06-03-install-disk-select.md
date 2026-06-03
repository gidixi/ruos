# 214 — install: selezione del disco (list + install <idx>)

**Data:** 2026-06-03

## Cosa
`install` senza argomenti ora ELENCA i dischi SATA (`[idx] modello (N MiB)`) senza
cancellare niente; `install <idx>` installa sul disco scelto. Prima auto-targetava
il primo SATA — pericoloso su una macchina con più dischi. La host fn `ruos_install`
prende ora `(esp_mib, target)`: target<0 = list, target>=0 = installa su quella
porta (con la guardia /mnt). Prerequisito per il test su hardware reale multi-disco.

## File toccati
- kernel/src/wasm/host/proc.rs (ruos_install + registrazione 2-arg)
- user/install/src/main.rs (parsing arg + messaggi), user-bin/install.wasm
- user-bin/m2b2-init.sh (install -> install 0)
- tests/m2b2-test.sh (fase 2: attende il marker init `m2b2-installed` invece di
  `mnt mounted FAT` — quest'ultimo è emesso dal kernel prima che init giri,
  causando una corsa che uccideva QEMU prima di `ruos boot OK`)

## Verifica
make run-m2b2-test → TEST_PASS_M2B2 (install 0 autora+copia, boot da SSD OVMF).
`install` (no arg) elenca il disco: `INFO install [0] "QEMU HARDDISK" (64 MiB)`,
nessuna scrittura (no WIPING/format/gpt). make run-test → TEST_PASS.
