# 03 — Frame allocator fisico (bitmap, da E820)

**Data:** 2026-05-27

## Cosa
- Implementato frame allocator a bitmap su [0, 4 GiB), inizializzato dalla E820
  map a 0x4000; riserva i primi 24 MiB.
- API: allocFrame/allocFrames/freeFrame/freeFrameCount.
- initFrameAllocator() chiamato per primo in main(); test in memTest.

## Perché
Layer 1 del gestore memoria: traccia la RAM fisica a granularità 4 KiB. Base per
paging e heap.

## File toccati
- x64barebones/Kernel/include/frameAllocator.h
- x64barebones/Kernel/memory/frameAllocator.c
- x64barebones/Kernel/memory/memTest.c
- x64barebones/Kernel/kernel.c
- CHANGELOG/03-26-05-27-frame-allocator.md
