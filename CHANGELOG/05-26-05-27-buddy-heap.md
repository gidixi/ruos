# 05 — Heap kernel (buddy allocator)

**Data:** 2026-05-27

## Cosa
- Buddy allocator su [16 MiB, 24 MiB) (8 MiB), blocchi 32 B..8 MiB.
- kmalloc/kfree con split e coalescing dei buddy; heapFreeBytes per i test.
- initHeap() chiamato in main dopo initPaging; test in memTest.

## Perché
Layer 3 del gestore memoria: heap vero con free funzionante. Serve a #3
(filesystem) e rimpiazza il bump allocator.

## File toccati
- x64barebones/Kernel/include/heap.h
- x64barebones/Kernel/memory/heap.c
- x64barebones/Kernel/memory/memTest.c
- x64barebones/Kernel/kernel.c
- CHANGELOG/05-26-05-27-buddy-heap.md
