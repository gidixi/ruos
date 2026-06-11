# 439 — init: load_from_disk /mnt/etc/units (yaml+json, robusto a errori)

**Data:** 2026-06-11

## Cosa
`service/mod.rs`: `load_from_disk` — `vfs::readdir(UNITS_DIR)`, parse per
estensione (.yaml/.yml/.json), `unitfile::build`, duplicati skippati con
warn, file malformato = log + skip (le altre unit proseguono), dir
assente = solo builtin. Timer armati al load (`next_fire =
compute_next`; BootPlus = tick assoluto da boot).

## Perché
Fase 7 spec init-units: config persistente su FAT32.

## File toccati
- kernel/src/service/mod.rs
