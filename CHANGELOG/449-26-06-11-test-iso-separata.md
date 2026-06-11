# 449 — Makefile: ISO di test separata (os-test.iso), os.iso sempre pulita

**Data:** 2026-06-11

## Cosa

Nuova variabile `TEST_ISO := build/os-test.iso`. `run-test` e `run-test-usb`
buildano e bootano quella (`make iso INIT_SCRIPT=user-bin/smoke.sh
ISO=$(TEST_ISO)`) invece di sovrascrivere `build/os.iso`.

## Perché

`make run-test` ri-buildava `build/os.iso` con `smoke.sh` come `/etc/init.sh`:
dopo un run-test la ISO "buona" conteneva la batteria di smoke test e, se
flashata su hardware reale, bootava eseguendo tutti i test (rtop, ping, FAT
write, ecc.) prima della shell. Ora `os.iso` esce solo da `make iso` con
l'init minimale; i target di test non la toccano più. (Gli script in `tests/`
specializzati buildano ancora `os.iso` con init propri — invariati.)

## File toccati

- Makefile
