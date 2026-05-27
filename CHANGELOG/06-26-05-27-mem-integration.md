# 06 — Integrazione: syscall memoria → heap, malloc/free, comando memtest

**Data:** 2026-05-27

## Cosa
- systemCalls.c: memoryManagement usa kmalloc/kfree (free non più no-op);
  rimosso bump allocator 0x900000. Aggiunta syscall SYS_CALL_MEMTEST.
- Userland: memoryManagement passa il puntatore; malloc/free aggiornati;
  aggiunto wrapper memTest e comando shell "memtest".
- Disattivato il self-test al boot (MEM_TEST_ON_BOOT 0); test ora on-demand.

## Perché
Collega il nuovo gestore memoria al resto del sistema e dà un free funzionante,
chiudendo il sotto-progetto #1.

## File toccati
- x64barebones/Kernel/systemCalls.c
- x64barebones/Userland/SampleCodeModule/systemCalls.c
- x64barebones/Userland/SampleCodeModule/include/stdlib.h
- x64barebones/Userland/SampleCodeModule/stdlib.c
- x64barebones/Userland/SampleCodeModule/shell.c
- x64barebones/Kernel/kernel.c
- CHANGELOG/06-26-05-27-mem-integration.md
