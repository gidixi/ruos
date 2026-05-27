# Memory Manager — Design Spec

**Date:** 2026-05-27
**Sub-project:** #1 of the MinimalOS evolution (Memory Manager)
**Status:** Approved design, ready for implementation planning

## Context

MinimalOS is an x86-64 hobby OS built on the Pure64 bootloader, currently running
in QEMU. The goal is to evolve it into a fuller OS that boots on real x86-64
hardware via USB. The work is decomposed into five sub-projects, built in this
dependency order:

1. **Memory manager** (this spec) — foundations.
2. Multitasking — preemptive scheduler, processes, syscalls. Depends on #1.
3. Disk driver + filesystem (ATA/AHCI + read/write FS). Depends on #1.
4. Real-hardware portability (VESA video, robust keyboard, hardware detection).
   Partly parallel.
5. Shell commands exposing the new capabilities. Depends on #1–#4.

This spec covers **only sub-project #1**. Each later sub-project gets its own
spec → plan → implementation cycle.

### Why a new memory manager

The current kernel allocates memory with a bump pointer at `0x900000`
(`systemCalls.c`, `memoryManagement`) and `free` is a no-op. There is no physical
frame tracking, no per-process address spaces, and no real heap. Multitasking (#2)
needs per-process isolation, and the filesystem (#3) needs a real heap, so a proper
memory manager must come first.

### What Pure64 already provides

Verified by reading the bootloader source:

- **E820 memory map** at physical `0x4000` — BIOS-reported usable RAM ranges.
- **`mem_amount`** (total usable RAM in MiB) at `SystemVariables + 132`.
- **Paging already enabled.** PML4 at `0x2000`, page-directory tables at
  `0x10000`–`0x4FFFF`, using **2 MiB pages**, identity-mapping the low 4 GiB (with
  room for 64 GiB), plus a higher-half mapping at `0xFFFF800000000000`.
- VESA framebuffer info at `0x5C00` and `os_VideoBase` (relevant later for #4).

The frame allocator will initialize from the E820 map, so it works on real
hardware, not just a fixed QEMU configuration.

## Goals

- Track physical memory at 4 KiB frame granularity, initialized from E820.
- Provide a paging API for fine-grained 4 KiB mapping and per-process address
  spaces, building on Pure64's existing kernel page tables.
- Provide a real kernel heap (`kmalloc`/`kfree`) using a buddy allocator.
- Replace the bump allocator so `free` actually reclaims memory.

## Non-goals (YAGNI)

- Per-process address space *creation/teardown logic* belongs to #2; this spec
  only provides the paging API #2 will call.
- No demand paging, swapping, or copy-on-write.
- No slab allocator.
- No userland `malloc`/`free` library work beyond wiring the existing memory
  syscall to the new heap.

## Architecture

Three layers, bottom to top:

```
┌─────────────────────────────────────┐
│  Kernel heap (buddy)  kmalloc/kfree  │  layer 3
├─────────────────────────────────────┤
│  Paging  map/unmap/createAddrSpace   │  layer 2  (4 KiB pages)
├─────────────────────────────────────┤
│  Physical frame allocator (bitmap)   │  layer 1
└─────────────────────────────────────┘
        ↑ initialized from E820 map @ 0x4000
```

Each layer depends only on the layer below it. The frame allocator knows nothing
about paging or the heap; the heap requests frames through the frame allocator and
maps them through paging.

## Components

### 1. Physical frame allocator — `memory/frameAllocator.c` / `.h`

- **Data structure:** bitmap, 1 bit per 4 KiB frame (0 = free, 1 = used).
- **Initialization:** read the E820 map at physical `0x4000` and `mem_amount` to
  size the bitmap and mark which frames exist and are usable.
- **Reserved regions marked as used at init:**
  - Low memory below 1 MiB (`0x0`–`0xFFFFF`).
  - Pure64 page tables (`0x2000`–`0x4FFFF`).
  - The kernel image (`text`/`rodata`/`data`/`bss` through `endOfKernel`).
  - Loaded modules at `0x400000` (code) and `0x500000` (data).
  - The kernel stack region.
  - The static RTL8139 buffers.
  - The frame allocator's own bitmap storage.
- **API:**
  - `uint64_t allocFrame(void)` — returns physical address of a free frame, or
    `0` if none available.
  - `void freeFrame(uint64_t physAddr)` — marks a frame free.
  - `uint64_t allocFrames(uint64_t n)` — returns physical address of `n`
    contiguous free frames, or `0` if none.

### 2. Paging — `memory/paging.c` / `.h`

- **Page size:** 4 KiB, finer than Pure64's 2 MiB pages, for fine-grained map/unmap.
- **Kernel mappings** stay on Pure64's existing tables.
- **API:**
  - `int mapPage(uint64_t *pml4, uint64_t virt, uint64_t phys, uint64_t flags)` —
    maps a virtual page to a physical frame; allocates intermediate page tables
    on demand. Returns non-zero on failure (e.g., out of frames for a table).
  - `void unmapPage(uint64_t *pml4, uint64_t virt)` — removes a mapping and
    invalidates the TLB entry.
  - `uint64_t *createAddressSpace(void)` — allocates a new PML4 and copies the
    kernel mappings into it; returns the PML4 pointer (or `NULL` on failure).
    Consumed by #2.
  - `void switchAddressSpace(uint64_t *pml4)` — loads CR3 with the given PML4.

### 3. Kernel heap (buddy allocator) — `memory/heap.c` / `.h`

- **Managed region:** placed above the loaded modules. Initial base candidate
  `0x1000000` (16 MiB); exact base/size finalized during planning against the
  reserved-region list. Backing frames come from the frame allocator.
- **Algorithm:** buddy system. Blocks are powers of two; allocation splits a
  larger free block down to the needed order; freeing merges with the buddy when
  free.
- **API:**
  - `void *kmalloc(uint64_t size)` — returns a pointer to a block of at least
    `size` bytes, or `NULL` if none available.
  - `void kfree(void *ptr)` — frees a block and coalesces buddies.

### 4. Integration

- `systemCalls.c` `memoryManagement` switches from the bump pointer at `0x900000`
  to `kmalloc`/`kfree`. The `MEMORY_FREE_CODE` path now actually frees instead of
  being a no-op.
- `kernel.c` `main()` calls, in order, before initializing interrupts and drivers:
  `initFrameAllocator()` → `initPaging()` → `initHeap()`.

## Error handling

- `allocFrame` / `allocFrames` return `0` when memory is exhausted.
- `kmalloc` returns `NULL` when no block is available; callers must check.
- `mapPage` allocates intermediate page tables on demand; if a frame for a table
  cannot be allocated it returns a non-zero error and leaves no partial mapping.

## Testing

Bare-metal testing is constrained, so the strategy is an in-kernel self-test:

- `memTest()` runs at boot behind a debug flag. It allocates and frees frames and
  heap blocks in patterns and asserts:
  - the frame bitmap stays consistent (no double-allocation, no overlap with
    reserved regions);
  - buddy split/merge is correct (freeing then reallocating returns coherent
    blocks; full free returns the heap to its initial state);
  - `mapPage`/`unmapPage` round-trip (a mapped virtual address reads/writes the
    expected frame; after unmap it is gone).
- Results print via `naiveConsole`.
- A shell `memtest` command re-runs the self-test on demand.

## Open items for the implementation plan

- Finalize the exact heap base and size against the full reserved-region list.
- Decide buddy min/max block orders.
- Confirm the kernel-mapping copy strategy in `createAddressSpace` (share kernel
  PDPT/PD entries vs. deep copy).
