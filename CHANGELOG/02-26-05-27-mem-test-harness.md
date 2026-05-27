# 02 — Harness self-test memoria (seriale) + build memory/

**Data:** 2026-05-27

## Cosa
- Kernel/Makefile compila e linka memory/*.c.
- Aggiunto serial logger COM1 + driver test (memTest) in Kernel/memory/memTest.c.
- main() esegue memTest al boot dietro MEM_TEST_ON_BOOT e esce via isa-debug-exit.
- Aggiunto runtest.sh per lanciare QEMU catturando l'output seriale.

## Perché
Serve infrastruttura di test osservabile su bare-metal prima di implementare i
componenti del gestore memoria (TDD).

## Nota
memTest() viene eseguito in main() PRIMA di initRTL(): sotto QEMU headless il BAR
I/O della rtl8139 resta non assegnato, quindi initRTL() resta in loop sul reset
bit della scheda (porta I/O hardcoded 0xC000) e non raggiungerebbe mai memTest.
Eseguendo i test prima e uscendo via isa-debug-exit l'harness resta osservabile
senza toccare il codice di rete (fuori scope di questo task).

## File toccati
- x64barebones/Kernel/Makefile
- x64barebones/Kernel/include/memTest.h
- x64barebones/Kernel/memory/memTest.c
- x64barebones/Kernel/kernel.c
- x64barebones/runtest.sh
- CHANGELOG/02-26-05-27-mem-test-harness.md
