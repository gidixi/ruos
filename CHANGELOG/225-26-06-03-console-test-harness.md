# 225 — Console engine self-test harness

**Data:** 2026-06-03

## Cosa
Aggiunto il modulo `kernel/src/console/engine_test.rs` con la funzione `run()`
che esegue asserzioni in-kernel e stampa `CONSOLE_TEST: OK` (o `FAIL:<id>`)
su seriale. Dichiarato come `pub mod engine_test` in `console/mod.rs`. Invocato
alla fine della fase `boot::phases::devices::init()` in modo incondizionato
(dopo il branch framebuffer). Aggiunto lo script `user-bin/console-test-init.sh`
(chiama `poweroff` subito) e il target `make run-console-test` nel Makefile,
che builda la ISO con lo script di init minimo, avvia QEMU headless e asserisce
`CONSOLE_TEST: OK` sul seriale.

## Perché
Task 1 del piano terminal-engine: stabilisce il ciclo harness/boot-hook/marker/grep
prima di implementare le feature reali. I task successivi aggiungeranno asserzioni
a `run_inner()` senza toccare l'infrastruttura.

## File toccati
- kernel/src/console/engine_test.rs
- kernel/src/console/mod.rs
- kernel/src/boot/phases/devices.rs
- user-bin/console-test-init.sh
- Makefile
