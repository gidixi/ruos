# 347 — Live-CD: fix di integrazione (multi-HBA, Rock Ridge, mount_all)

**Data:** 2026-06-08

## Cosa
Tre fix emersi testando il boot live-CD in QEMU (q35):
- **Multi-HBA**: in q35 il CD-ROM ATAPI sta sul controller AHCI builtin (ICH9),
  diverso da quello col disco SATA che `find_and_init` inizializza. Aggiunti
  `Hba::init_dev` + `Hba::find_all_except` e reso `ahci::acquire_atapi_port`
  scan di TUTTI i controller AHCI (boot HBA prima, poi gli altri).
- **Rock Ridge**: l'ISO di Limine/xorriso ha nomi 8.3 mangled nell'ISO9660
  primario (`SHELL.WAS;1`) e i nomi reali (`shell.wasm`) nell'entry SUSP `NM`.
  `iso9660::parse_dir` ora legge il nome Rock Ridge `NM`, con fallback al nome 8.3.
- **mount_all sempre**: la fase storage carica SEMPRE i moduli Limine (init.sh,
  init.wasm, /root) e poi SOVRAPPONE `/bin` dal CD (shadow del /bin tmpfs),
  invece di saltare mount_all quando il CD c'è (rompeva /etc/init.sh).

## Perché
Senza questi, il CD non veniva trovato (porta su HBA diverso), i file non si
risolvevano (nome 8.3 != nome reale) e lo smoke test falliva (init.sh mancante).
Con i fix: `make run-test` → TEST_PASS, `/bin` letto on-demand dal CD ATAPI.

## File toccati
- kernel/src/ahci/hba.rs
- kernel/src/ahci/mod.rs
- kernel/src/vfs/iso9660.rs
- kernel/src/boot/phases/storage.rs
- CHANGELOG/347-26-06-08-livecd-integration-fixes.md
