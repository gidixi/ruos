# 160 — split init.sh in init.sh (minimo) + smoke.sh (assert)

**Data:** 2026-05-30

## Cosa
- `user-bin/init.sh`: ridotto a un banner di una riga (`echo ruos boot OK`).
  Boot interattivo passa dal prompt shell in pochi secondi invece di
  ~80s.
- `user-bin/smoke.sh`: nuovo, contiene la batteria completa di smoke
  test (coreutils, FAT R/W, ping, pipe). Stessa sequenza precedente di
  init.sh, niente comportamenti nuovi.
- `Makefile`: variabile `INIT_SCRIPT ?= user-bin/init.sh` parametrizza
  cosa viene copiato come `/etc/init.sh` sull'ISO. `make iso` / `make
  run` usano il minimo; `make run-test` fa `$(MAKE) iso
  INIT_SCRIPT=user-bin/smoke.sh` per la batteria completa.
- Anche `test-boot` usa `$(INIT_SCRIPT)` per uniformità (default minimo;
  l'assert "smoke" del target arriva dal feature `boot-checks`, non
  da init.sh, quindi resta verde).

## Perché
Su VirtualBox e su `make run` interattivo, far girare 30+ tool `.wasm`
prima di vedere il prompt era ~80s di attesa solo per il "self-check"
del boot. Lo split mantiene il sentinel `shell: init.sh complete` —
quindi `run-test` resta verde — ma sblocca il caso d'uso "voglio solo
provare lo shell o testare SSH/pipe interattivamente" con un boot
quasi istantaneo.

`make run-test` rigenera la ISO con smoke.sh e poi va in QEMU
(TEST_PASS verificato).

## File toccati
- user-bin/init.sh
- user-bin/smoke.sh
- Makefile
