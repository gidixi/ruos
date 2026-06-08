# 348 — Live-CD: bin off-boot (rimossi i moduli /bin da limine.conf)

**Data:** 2026-06-08

## Cosa
Rimosse tutte le coppie `module_path/module_cmdline` di `/bin/*` da `limine.conf`.
Limine ora carica solo: kernel, init.wasm, /root demo, /etc/init.sh, /payload/*.
I bin restano sul filesystem ISO9660 (il Makefile li copia già in `iso_root/bin/`)
e vengono letti on-demand dal CD via ATAPI/ISO9660.

## Perché
Boot più elegante e meno RAM: prima Limine caricava 61 moduli (~45 MB di app
.cwasm + tool CLI) in RAM e li ricopiava in tmpfs. Ora ne carica 2 (init).
Verificato: `make run-test` → TEST_PASS, log `mounted 2 boot modules` +
`/bin overlaid from ISO9660 (ATAPI)`, `/bin/ls.wasm` eseguito dal CD.

## File toccati
- limine.conf
- CHANGELOG/348-26-06-08-livecd-bins-off-boot.md
