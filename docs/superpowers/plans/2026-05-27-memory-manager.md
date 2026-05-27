# Memory Manager Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a real memory manager to the kernel — physical frame allocator (bitmap from E820), 4 KiB paging API, and a buddy-allocator kernel heap — replacing the bump allocator.

**Architecture:** Three layers built bottom-up: frame allocator (layer 1) → paging (layer 2) → buddy heap (layer 3). Each layer depends only on the one below. Verified by an in-kernel self-test that prints PASS/FAIL over serial (COM1), runnable at boot (TDD) and later via a `memtest` shell command.

**Tech Stack:** C99 freestanding (`-ffreestanding -nostdlib`), NASM, GNU ld, QEMU. Existing port I/O helpers `sysInByte`/`sysOutByte` from `asm/sysIO.asm`.

---

## Key facts (verified against the codebase)

- **Build:** from `x64barebones/`, `make all` builds bootloader + kernel + userland + image.
- **Kernel Makefile** globs only top-level `*.c`. New `memory/*.c` files require a Makefile change (Task 1).
- **Headers** live in `Kernel/include/` and are found via `-I./include`.
- **E820 map** at physical `0x4000`. Pure64 stores **32-byte entries** (`base` u64, `length` u64, `type` u32, `acpi` u32, then 8 bytes padding), terminated by an all-zero record (`type == 0`). `type == 1` means usable RAM.
- **Paging:** Pure64 enables paging, PML4 at `0x2000`, identity-maps the low 4 GiB with **2 MiB pages**. So `mapPage` must only target virtual ranges *not* already covered by those 2 MiB pages — tests use `0x4000000000` (256 GiB), which is unmapped.
- **Boot flow:** `loader.asm` calls `initializeKernelBinary()` then `main()` (`kernel.c`). New init calls go at the top of `main()`.
- **Identity map invariant:** any physical address `< 4 GiB` is identity-mapped, so a physical frame address is directly usable as a pointer. The frame allocator caps at 4 GiB for this reason; the heap and all page-table frames live in this range.

## Physical layout decisions (locked)

- `PAGE_SIZE` = `0x1000` (4 KiB).
- Frame allocator tracks `[0, 4 GiB)` → 1,048,576 frames → 131,072-byte bitmap (lives in BSS, zeroed at boot).
- **Reserved (never handed out):** `[0, 0x1800000)` (low 24 MiB) — covers low memory, Pure64 page tables, the kernel image, loaded modules (`0x400000`, `0x500000`), the kernel stack, the static RTL8139 buffers, and the bitmap itself.
- **Heap region:** `[0x1000000, 0x1800000)` (16–24 MiB, 8 MiB). It sits inside the reserved low region, so the frame allocator never hands out heap frames. The buddy allocator owns this region directly (identity-mapped → usable pointers).
- **Frame allocator hands out** usable E820 frames in `[0x1800000, 4 GiB)` — used for process page tables and process pages (consumed by sub-project #2).

## File structure

- `Kernel/include/frameAllocator.h` — frame allocator API.
- `Kernel/memory/frameAllocator.c` — bitmap physical frame allocator.
- `Kernel/include/paging.h` — paging API + page flags.
- `Kernel/memory/paging.c` — 4 KiB page-table walk, map/unmap, address spaces.
- `Kernel/include/heap.h` — heap API.
- `Kernel/memory/heap.c` — buddy allocator.
- `Kernel/include/memTest.h` — self-test entry point.
- `Kernel/memory/memTest.c` — serial logger + self-tests.
- `Kernel/Makefile` — compile/link `memory/*.c` (modify).
- `Kernel/kernel.c` — add init calls + boot-time self-test (modify).
- `Kernel/systemCalls.c` — wire memory syscall to heap (modify).
- `Userland/SampleCodeModule/systemCalls.c`, `stdlib.c`, `include/stdlib.h` — pass pointer to free, add memtest syscall (modify).
- `Userland/SampleCodeModule/shell.c` — add `memtest` command (modify).
- `x64barebones/runtest.sh` — QEMU launch capturing serial (create).

---

## Task 1: Build infrastructure + serial self-test harness + boot hook

**Files:**
- Modify: `x64barebones/Kernel/Makefile`
- Create: `x64barebones/Kernel/include/memTest.h`
- Create: `x64barebones/Kernel/memory/memTest.c`
- Modify: `x64barebones/Kernel/kernel.c`
- Create: `x64barebones/runtest.sh`
- Create: `CHANGELOG/02-26-05-27-mem-test-harness.md`

- [ ] **Step 1: Make the kernel Makefile compile `memory/*.c`**

Edit `x64barebones/Kernel/Makefile`. After the `SOURCES_ASM` line add a memory sources var, and add its objects to the link. Replace lines 4-7:

```make
KERNEL=kernel.bin
SOURCES=$(wildcard *.c)
SOURCES_MEM=$(wildcard memory/*.c)
SOURCES_ASM=$(wildcard asm/*.asm)
OBJECTS=$(SOURCES:.c=.o)
OBJECTS_MEM=$(SOURCES_MEM:.c=.o)
OBJECTS_ASM=$(SOURCES_ASM:.asm=.o)
```

Change the link line (was line 16) to include `$(OBJECTS_MEM)`:

```make
$(KERNEL): $(LOADEROBJECT) $(OBJECTS) $(OBJECTS_MEM) $(STATICLIBS) $(OBJECTS_ASM)
	$(LD) $(LDFLAGS) -T kernel.ld -o $(KERNEL) $(LOADEROBJECT) $(OBJECTS) $(OBJECTS_MEM) $(OBJECTS_ASM) $(STATICLIBS)
```

Change the `clean` rule to remove memory objects:

```make
clean:
	rm -rf asm/*.o memory/*.o *.o *.bin
```

The existing `%.o: %.c` rule already matches `memory/x.c` → `memory/x.o`.

- [ ] **Step 2: Create the self-test header**

Create `x64barebones/Kernel/include/memTest.h`:

```c
#ifndef _MEM_TEST_H
#define _MEM_TEST_H

void memTest(void);   // runs all memory self-tests, logs PASS/FAIL over COM1

#endif
```

- [ ] **Step 3: Create the serial logger + empty test driver**

Create `x64barebones/Kernel/memory/memTest.c`:

```c
#include <memTest.h>
#include <sysIO.h>
#include <stdint.h>

#define COM1 0x3F8

static int failCount = 0;

static void serialInit(void) {
	sysOutByte(COM1 + 1, 0x00);   // disable interrupts
	sysOutByte(COM1 + 3, 0x80);   // enable DLAB
	sysOutByte(COM1 + 0, 0x03);   // divisor low (38400 baud)
	sysOutByte(COM1 + 1, 0x00);   // divisor high
	sysOutByte(COM1 + 3, 0x03);   // 8N1, DLAB off
	sysOutByte(COM1 + 2, 0xC7);   // enable FIFO, clear, 14-byte threshold
	sysOutByte(COM1 + 4, 0x0B);   // IRQs enabled, RTS/DSR set
}

static void serialPutc(char c) {
	while ((sysInByte(COM1 + 5) & 0x20) == 0);   // wait for transmit-empty
	sysOutByte(COM1, (uint8_t) c);
}

static void serialPrint(const char * s) {
	while (*s) serialPutc(*s++);
}

#define ASSERT(cond, name) do {                       \
		serialPrint(name);                            \
		if (cond) { serialPrint(": PASS\n"); }        \
		else { serialPrint(": FAIL\n"); failCount++; }\
	} while (0)

void memTest(void) {
	failCount = 0;
	serialInit();
	serialPrint("=== memTest start ===\n");
	/* component tests added in later tasks */
	serialPrint(failCount == 0 ? "=== memTest: ALL PASS ===\n"
	                           : "=== memTest: FAILURES ===\n");
}
```

- [ ] **Step 4: Hook the self-test into boot**

Edit `x64barebones/Kernel/kernel.c`. Add include near the other includes (after line 13):

```c
#include <memTest.h>
```

Add a boot-test toggle just below the includes (after line 13 block):

```c
#define MEM_TEST_ON_BOOT 1   /* set to 0 for release builds */
```

Replace `main()` (lines 91-101) with:

```c
int main()
{
	initializeInterruptions();
	activeRTLdma();
	initRTL();

#if MEM_TEST_ON_BOOT
	memTest();
	sysOutByte(0xF4, 0x00);   /* isa-debug-exit: stop QEMU after tests */
#endif

	ncClear();
	((EntryPoint)sampleCodeModuleAddress)();
	return 0;
}
```

Add `#include <sysIO.h>` to the includes if not present (it is not currently — add after line 13).

- [ ] **Step 5: Create the test launcher script**

Create `x64barebones/runtest.sh`:

```bash
#!/bin/bash
# Builds nothing; run `make all` first. Boots the image, runs memTest,
# prints serial output to this terminal, and exits via isa-debug-exit.
qemu-system-x86_64 -hda Image/x64BareBonesImage.qcow2 -m 512 \
	-serial stdio -display none -no-reboot \
	-device isa-debug-exit,iobase=0xf4,iosize=0x04 \
	-device rtl8139,netdev=n0,mac=DE:00:40:AA:21:2E -netdev user,id=n0
```

- [ ] **Step 6: Build and run the empty harness**

Run:
```bash
cd x64barebones && make all && chmod +x runtest.sh && ./runtest.sh
```
Expected serial output:
```
=== memTest start ===
=== memTest: ALL PASS ===
```
QEMU exits on its own (isa-debug-exit). If `make` fails on the `memory/` rule, recheck Step 1.

- [ ] **Step 7: Write the changelog entry**

Create `CHANGELOG/02-26-05-27-mem-test-harness.md`:

```markdown
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

## File toccati
- x64barebones/Kernel/Makefile
- x64barebones/Kernel/include/memTest.h
- x64barebones/Kernel/memory/memTest.c
- x64barebones/Kernel/kernel.c
- x64barebones/runtest.sh
- CHANGELOG/02-26-05-27-mem-test-harness.md
```

- [ ] **Step 8: Commit**

```bash
git add x64barebones/Kernel/Makefile x64barebones/Kernel/include/memTest.h \
        x64barebones/Kernel/memory/memTest.c x64barebones/Kernel/kernel.c \
        x64barebones/runtest.sh CHANGELOG/02-26-05-27-mem-test-harness.md
git commit -m "feat(mem): serial self-test harness and memory/ build support"
```

---

## Task 2: Physical frame allocator

**Files:**
- Create: `x64barebones/Kernel/include/frameAllocator.h`
- Create: `x64barebones/Kernel/memory/frameAllocator.c`
- Modify: `x64barebones/Kernel/memory/memTest.c`
- Modify: `x64barebones/Kernel/kernel.c`
- Create: `CHANGELOG/03-26-05-27-frame-allocator.md`

- [ ] **Step 1: Create the header**

Create `x64barebones/Kernel/include/frameAllocator.h`:

```c
#ifndef _FRAME_ALLOCATOR_H
#define _FRAME_ALLOCATOR_H

#include <stdint.h>

void     initFrameAllocator(void);
uint64_t allocFrame(void);              /* physical addr of a free 4 KiB frame, or 0 */
uint64_t allocFrames(uint64_t n);       /* physical addr of n contiguous frames, or 0 */
void     freeFrame(uint64_t physAddr);
uint64_t freeFrameCount(void);          /* number of free frames (for tests) */

#endif
```

- [ ] **Step 2: Write the failing test**

Edit `x64barebones/Kernel/memory/memTest.c`. Add include at top:

```c
#include <frameAllocator.h>
```

Add this function above `memTest`:

```c
static void testFrameAllocator(void) {
	uint64_t a = allocFrame();
	ASSERT(a != 0, "frame.alloc.nonzero");
	ASSERT((a % 0x1000) == 0, "frame.alloc.aligned");
	ASSERT(a >= 0x1800000, "frame.alloc.above-reserved");

	uint64_t b = allocFrame();
	ASSERT(b != a, "frame.alloc.distinct");

	uint64_t before = freeFrameCount();
	freeFrame(a);
	freeFrame(b);
	ASSERT(freeFrameCount() == before + 2, "frame.free.count");

	uint64_t c = allocFrames(4);
	ASSERT(c != 0, "frame.allocFrames.nonzero");
	ASSERT((c % 0x1000) == 0, "frame.allocFrames.aligned");
	freeFrame(c);
	freeFrame(c + 0x1000);
	freeFrame(c + 0x2000);
	freeFrame(c + 0x3000);
}
```

Call it inside `memTest`, replacing the `/* component tests added in later tasks */` comment:

```c
	testFrameAllocator();
```

- [ ] **Step 3: Build to verify it fails**

Run:
```bash
cd x64barebones && make all
```
Expected: **link error** — `undefined reference to 'allocFrame'` (and friends). This is the red state.

- [ ] **Step 4: Implement the frame allocator**

Create `x64barebones/Kernel/memory/frameAllocator.c`:

```c
#include <frameAllocator.h>
#include <stdint.h>

#define PAGE_SIZE        0x1000ULL
#define MAX_PHYS_BYTES   (4ULL * 1024 * 1024 * 1024)   /* 4 GiB, matches identity map */
#define MAX_FRAMES       (MAX_PHYS_BYTES / PAGE_SIZE)   /* 1048576 */
#define BITMAP_BYTES     (MAX_FRAMES / 8)               /* 131072 */

#define KERNEL_RESERVED_END 0x1800000ULL                /* 24 MiB: low mem + kernel + modules + heap */

#define E820_MAP_ADDR    0x4000ULL
#define E820_TYPE_USABLE 1

typedef struct {
	uint64_t base;
	uint64_t length;
	uint32_t type;
	uint32_t acpi;
	uint64_t pad;
} __attribute__((packed)) E820Entry;

static uint8_t  bitmap[BITMAP_BYTES];   /* 0 = free, 1 = used; BSS-zeroed */
static uint64_t usedFrames = 0;

static int  isUsed(uint64_t f)   { return bitmap[f / 8] & (1 << (f % 8)); }
static void markUsed(uint64_t f) { bitmap[f / 8] |=  (1 << (f % 8)); }
static void markFree(uint64_t f) { bitmap[f / 8] &= ~(1 << (f % 8)); }

static void reserveRange(uint64_t start, uint64_t end) {
	uint64_t first = start / PAGE_SIZE;
	uint64_t last  = (end + PAGE_SIZE - 1) / PAGE_SIZE;
	for (uint64_t f = first; f < last && f < MAX_FRAMES; f++) {
		if (!isUsed(f)) { markUsed(f); usedFrames++; }
	}
}

void initFrameAllocator(void) {
	/* Start with everything marked used. */
	for (uint64_t i = 0; i < BITMAP_BYTES; i++) bitmap[i] = 0xFF;
	usedFrames = MAX_FRAMES;

	/* Free frames covered by usable E820 regions. */
	E820Entry * e = (E820Entry *) E820_MAP_ADDR;
	while (!(e->base == 0 && e->length == 0 && e->type == 0)) {
		if (e->type == E820_TYPE_USABLE) {
			uint64_t start = e->base;
			uint64_t end   = e->base + e->length;
			if (end > MAX_PHYS_BYTES) end = MAX_PHYS_BYTES;
			uint64_t f    = (start + PAGE_SIZE - 1) / PAGE_SIZE;  /* round up */
			uint64_t last = end / PAGE_SIZE;                       /* round down */
			for (; f < last && f < MAX_FRAMES; f++) {
				if (isUsed(f)) { markFree(f); usedFrames--; }
			}
		}
		e++;
	}

	/* Re-reserve the low 24 MiB (kernel, modules, page tables, heap, bitmap). */
	reserveRange(0, KERNEL_RESERVED_END);
}

uint64_t allocFrame(void) {
	for (uint64_t f = 0; f < MAX_FRAMES; f++) {
		if (!isUsed(f)) {
			markUsed(f);
			usedFrames++;
			return f * PAGE_SIZE;
		}
	}
	return 0;
}

uint64_t allocFrames(uint64_t n) {
	if (n == 0) return 0;
	uint64_t run = 0, start = 0;
	for (uint64_t f = 0; f < MAX_FRAMES; f++) {
		if (!isUsed(f)) {
			if (run == 0) start = f;
			if (++run == n) {
				for (uint64_t i = start; i < start + n; i++) { markUsed(i); usedFrames++; }
				return start * PAGE_SIZE;
			}
		} else {
			run = 0;
		}
	}
	return 0;
}

void freeFrame(uint64_t physAddr) {
	uint64_t f = physAddr / PAGE_SIZE;
	if (f < MAX_FRAMES && isUsed(f)) { markFree(f); usedFrames--; }
}

uint64_t freeFrameCount(void) {
	return MAX_FRAMES - usedFrames;
}
```

- [ ] **Step 5: Call init at boot**

Edit `x64barebones/Kernel/kernel.c`. Add include after `#include <memTest.h>`:

```c
#include <frameAllocator.h>
```

In `main()`, add as the very first statement (before `initializeInterruptions();`):

```c
	initFrameAllocator();
```

- [ ] **Step 6: Build and run to verify it passes**

Run:
```bash
cd x64barebones && make all && ./runtest.sh
```
Expected serial output includes:
```
frame.alloc.nonzero: PASS
frame.alloc.aligned: PASS
frame.alloc.above-reserved: PASS
frame.alloc.distinct: PASS
frame.free.count: PASS
frame.allocFrames.nonzero: PASS
frame.allocFrames.aligned: PASS
=== memTest: ALL PASS ===
```

- [ ] **Step 7: Write the changelog entry**

Create `CHANGELOG/03-26-05-27-frame-allocator.md`:

```markdown
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
```

- [ ] **Step 8: Commit**

```bash
git add x64barebones/Kernel/include/frameAllocator.h \
        x64barebones/Kernel/memory/frameAllocator.c \
        x64barebones/Kernel/memory/memTest.c x64barebones/Kernel/kernel.c \
        CHANGELOG/03-26-05-27-frame-allocator.md
git commit -m "feat(mem): bitmap physical frame allocator from E820"
```

---

## Task 3: Paging API

**Files:**
- Create: `x64barebones/Kernel/include/paging.h`
- Create: `x64barebones/Kernel/memory/paging.c`
- Modify: `x64barebones/Kernel/memory/memTest.c`
- Modify: `x64barebones/Kernel/kernel.c`
- Create: `CHANGELOG/04-26-05-27-paging.md`

- [ ] **Step 1: Create the header**

Create `x64barebones/Kernel/include/paging.h`:

```c
#ifndef _PAGING_H
#define _PAGING_H

#include <stdint.h>

#define PAGE_PRESENT 0x1
#define PAGE_RW      0x2
#define PAGE_USER    0x4

void      initPaging(void);
int       mapPage(uint64_t * pml4, uint64_t virt, uint64_t phys, uint64_t flags); /* 0 ok, <0 fail */
void      unmapPage(uint64_t * pml4, uint64_t virt);
uint64_t *createAddressSpace(void);     /* new PML4 copying kernel mappings, or 0 */
void      switchAddressSpace(uint64_t * pml4);
uint64_t *currentPML4(void);            /* PML4 from CR3 */

#endif
```

- [ ] **Step 2: Write the failing test**

Edit `x64barebones/Kernel/memory/memTest.c`. Add includes:

```c
#include <paging.h>
```

Add this function above `memTest`:

```c
static void testPaging(void) {
	uint64_t * space = createAddressSpace();
	ASSERT(space != 0, "paging.createAddrSpace");

	uint64_t phys = allocFrame();
	ASSERT(phys != 0, "paging.frame");

	/* 256 GiB: inside PML4[0] but well beyond Pure64's identity-mapped 4 GiB,
	   so no 2 MiB page covers it. */
	uint64_t virt = 0x4000000000ULL;
	uint64_t * cur = currentPML4();

	int r = mapPage(cur, virt, phys, PAGE_RW);
	ASSERT(r == 0, "paging.map.ok");

	volatile uint64_t * p = (volatile uint64_t *) virt;
	*p = 0xCAFEBABEULL;
	ASSERT(*p == 0xCAFEBABEULL, "paging.map.rw");
	/* phys is < 4 GiB so it is identity-mapped: same physical memory. */
	ASSERT(*(volatile uint64_t *) phys == 0xCAFEBABEULL, "paging.map.backing");

	unmapPage(cur, virt);
	freeFrame(phys);
}
```

Call it in `memTest` after `testFrameAllocator();`:

```c
	testPaging();
```

- [ ] **Step 3: Build to verify it fails**

Run:
```bash
cd x64barebones && make all
```
Expected: **link error** — `undefined reference to 'createAddressSpace'` etc. Red state.

- [ ] **Step 4: Implement paging**

Create `x64barebones/Kernel/memory/paging.c`:

```c
#include <paging.h>
#include <frameAllocator.h>
#include <stdint.h>

#define ENTRY_ADDR_MASK 0x000FFFFFFFFFF000ULL

#define PML4_IDX(v) (((v) >> 39) & 0x1FF)
#define PDPT_IDX(v) (((v) >> 30) & 0x1FF)
#define PD_IDX(v)   (((v) >> 21) & 0x1FF)
#define PT_IDX(v)   (((v) >> 12) & 0x1FF)

static void invlpg(uint64_t virt) {
	__asm__ volatile("invlpg (%0)" :: "r"(virt) : "memory");
}

uint64_t * currentPML4(void) {
	uint64_t cr3;
	__asm__ volatile("mov %%cr3, %0" : "=r"(cr3));
	return (uint64_t *)(cr3 & ENTRY_ADDR_MASK);
}

/* Returns the next-level table pointer; allocates+zeroes it when create != 0. */
static uint64_t * getOrCreate(uint64_t * table, uint64_t idx, int create) {
	if (!(table[idx] & PAGE_PRESENT)) {
		if (!create) return 0;
		uint64_t frame = allocFrame();    /* < 4 GiB → identity-mapped, usable as ptr */
		if (!frame) return 0;
		uint64_t * t = (uint64_t *) frame;
		for (int i = 0; i < 512; i++) t[i] = 0;
		table[idx] = frame | PAGE_PRESENT | PAGE_RW | PAGE_USER;
	}
	return (uint64_t *)(table[idx] & ENTRY_ADDR_MASK);
}

void initPaging(void) {
	/* Pure64 already enabled paging; nothing to set up. Present for symmetry
	   and a hook point for later work. */
}

int mapPage(uint64_t * pml4, uint64_t virt, uint64_t phys, uint64_t flags) {
	uint64_t * pdpt = getOrCreate(pml4, PML4_IDX(virt), 1);
	if (!pdpt) return -1;
	uint64_t * pd = getOrCreate(pdpt, PDPT_IDX(virt), 1);
	if (!pd) return -1;
	uint64_t * pt = getOrCreate(pd, PD_IDX(virt), 1);
	if (!pt) return -1;
	pt[PT_IDX(virt)] = (phys & ENTRY_ADDR_MASK) | (flags & 0xFFF) | PAGE_PRESENT;
	invlpg(virt);
	return 0;
}

void unmapPage(uint64_t * pml4, uint64_t virt) {
	uint64_t * pdpt = getOrCreate(pml4, PML4_IDX(virt), 0);
	if (!pdpt) return;
	uint64_t * pd = getOrCreate(pdpt, PDPT_IDX(virt), 0);
	if (!pd) return;
	uint64_t * pt = getOrCreate(pd, PD_IDX(virt), 0);
	if (!pt) return;
	pt[PT_IDX(virt)] = 0;
	invlpg(virt);
}

uint64_t * createAddressSpace(void) {
	uint64_t frame = allocFrame();
	if (!frame) return 0;
	uint64_t * newPml4 = (uint64_t *) frame;
	uint64_t * cur = currentPML4();
	for (int i = 0; i < 512; i++) newPml4[i] = cur[i];   /* share kernel mappings */
	return newPml4;
}

void switchAddressSpace(uint64_t * pml4) {
	__asm__ volatile("mov %0, %%cr3" :: "r"((uint64_t) pml4) : "memory");
}
```

- [ ] **Step 5: Call init at boot**

Edit `x64barebones/Kernel/kernel.c`. Add include after `#include <frameAllocator.h>`:

```c
#include <paging.h>
```

In `main()`, add right after `initFrameAllocator();`:

```c
	initPaging();
```

- [ ] **Step 6: Build and run to verify it passes**

Run:
```bash
cd x64barebones && make all && ./runtest.sh
```
Expected serial output includes:
```
paging.createAddrSpace: PASS
paging.frame: PASS
paging.map.ok: PASS
paging.map.rw: PASS
paging.map.backing: PASS
=== memTest: ALL PASS ===
```

- [ ] **Step 7: Write the changelog entry**

Create `CHANGELOG/04-26-05-27-paging.md`:

```markdown
# 04 — API paging (pagine 4 KiB) + spazi di indirizzamento

**Data:** 2026-05-27

## Cosa
- mapPage/unmapPage su page table 4 livelli (4 KiB), creando tabelle intermedie
  dal frame allocator.
- createAddressSpace (nuovo PML4 che copia i mapping del kernel),
  switchAddressSpace (carica CR3), currentPML4.
- initPaging() chiamato in main dopo il frame allocator; test in memTest.

## Perché
Layer 2 del gestore memoria. Serve a #2 (multitasking) per isolare i processi.

## File toccati
- x64barebones/Kernel/include/paging.h
- x64barebones/Kernel/memory/paging.c
- x64barebones/Kernel/memory/memTest.c
- x64barebones/Kernel/kernel.c
- CHANGELOG/04-26-05-27-paging.md
```

- [ ] **Step 8: Commit**

```bash
git add x64barebones/Kernel/include/paging.h x64barebones/Kernel/memory/paging.c \
        x64barebones/Kernel/memory/memTest.c x64barebones/Kernel/kernel.c \
        CHANGELOG/04-26-05-27-paging.md
git commit -m "feat(mem): 4 KiB paging API and address spaces"
```

---

## Task 4: Buddy-allocator kernel heap

**Files:**
- Create: `x64barebones/Kernel/include/heap.h`
- Create: `x64barebones/Kernel/memory/heap.c`
- Modify: `x64barebones/Kernel/memory/memTest.c`
- Modify: `x64barebones/Kernel/kernel.c`
- Create: `CHANGELOG/05-26-05-27-buddy-heap.md`

- [ ] **Step 1: Create the header**

Create `x64barebones/Kernel/include/heap.h`:

```c
#ifndef _HEAP_H
#define _HEAP_H

#include <stdint.h>

void     initHeap(void);
void *   kmalloc(uint64_t size);
void     kfree(void * ptr);
uint64_t heapFreeBytes(void);   /* total free bytes across the buddy lists (for tests) */

#endif
```

- [ ] **Step 2: Write the failing test**

Edit `x64barebones/Kernel/memory/memTest.c`. Add include:

```c
#include <heap.h>
```

Add this function above `memTest`:

```c
static void testHeap(void) {
	uint64_t before = heapFreeBytes();
	ASSERT(before > 0, "heap.init.free");

	void * a = kmalloc(100);
	ASSERT(a != 0, "heap.kmalloc.nonzero");
	void * b = kmalloc(100);
	ASSERT(b != 0 && b != a, "heap.kmalloc.distinct");

	*(volatile uint64_t *) a = 0x1234;
	ASSERT(*(volatile uint64_t *) a == 0x1234, "heap.write");

	kfree(a);
	kfree(b);
	ASSERT(heapFreeBytes() == before, "heap.free.restores");

	void * big = kmalloc(0x100000);   /* 1 MiB */
	ASSERT(big != 0, "heap.kmalloc.large");
	kfree(big);
	ASSERT(heapFreeBytes() == before, "heap.free.large");
}
```

Call it in `memTest` after `testPaging();`:

```c
	testHeap();
```

- [ ] **Step 3: Build to verify it fails**

Run:
```bash
cd x64barebones && make all
```
Expected: **link error** — `undefined reference to 'kmalloc'` etc. Red state.

- [ ] **Step 4: Implement the buddy heap**

Create `x64barebones/Kernel/memory/heap.c`:

```c
#include <heap.h>
#include <stdint.h>

#define HEAP_BASE   0x1000000ULL    /* 16 MiB */
#define HEAP_SIZE   0x800000ULL     /* 8 MiB  */
#define MIN_ORDER   5               /* 2^5  = 32 bytes smallest block */
#define MAX_ORDER   23              /* 2^23 = 8 MiB  = whole heap      */
#define HEADER_SIZE 8               /* stores the block order          */

typedef struct FreeBlock {
	struct FreeBlock * next;
} FreeBlock;

static FreeBlock * freeLists[MAX_ORDER + 1];

static int sizeToOrder(uint64_t size) {
	uint64_t need = size + HEADER_SIZE;
	int o = MIN_ORDER;
	while ((1ULL << o) < need) o++;
	return o;
}

static void listPush(int order, void * block) {
	FreeBlock * b = (FreeBlock *) block;
	b->next = freeLists[order];
	freeLists[order] = b;
}

static void * listPop(int order) {
	FreeBlock * b = freeLists[order];
	if (b) freeLists[order] = b->next;
	return b;
}

static int listRemove(int order, void * block) {
	FreeBlock ** pp = &freeLists[order];
	while (*pp) {
		if ((void *) *pp == block) { *pp = (*pp)->next; return 1; }
		pp = &(*pp)->next;
	}
	return 0;
}

void initHeap(void) {
	for (int i = 0; i <= MAX_ORDER; i++) freeLists[i] = 0;
	listPush(MAX_ORDER, (void *) HEAP_BASE);   /* one free block covering the heap */
}

void * kmalloc(uint64_t size) {
	if (size == 0) return 0;
	int order = sizeToOrder(size);
	if (order > MAX_ORDER) return 0;

	int o = order;
	while (o <= MAX_ORDER && !freeLists[o]) o++;
	if (o > MAX_ORDER) return 0;               /* out of memory */

	void * block = listPop(o);
	while (o > order) {                        /* split down to requested order */
		o--;
		void * buddy = (void *)((uint64_t) block + (1ULL << o));
		listPush(o, buddy);
	}
	*(uint64_t *) block = (uint64_t) order;    /* header */
	return (void *)((uint64_t) block + HEADER_SIZE);
}

void kfree(void * ptr) {
	if (!ptr) return;
	void * block = (void *)((uint64_t) ptr - HEADER_SIZE);
	int order = (int) *(uint64_t *) block;

	while (order < MAX_ORDER) {
		uint64_t off = (uint64_t) block - HEAP_BASE;
		uint64_t buddyOff = off ^ (1ULL << order);
		void * buddy = (void *)(HEAP_BASE + buddyOff);
		if (!listRemove(order, buddy)) break;  /* buddy not free → stop merging */
		if (buddy < block) block = buddy;       /* keep the lower address */
		order++;
	}
	listPush(order, block);
}

uint64_t heapFreeBytes(void) {
	uint64_t total = 0;
	for (int o = MIN_ORDER; o <= MAX_ORDER; o++)
		for (FreeBlock * b = freeLists[o]; b; b = b->next)
			total += (1ULL << o);
	return total;
}
```

- [ ] **Step 5: Call init at boot**

Edit `x64barebones/Kernel/kernel.c`. Add include after `#include <paging.h>`:

```c
#include <heap.h>
```

In `main()`, add right after `initPaging();`:

```c
	initHeap();
```

- [ ] **Step 6: Build and run to verify it passes**

Run:
```bash
cd x64barebones && make all && ./runtest.sh
```
Expected serial output includes:
```
heap.init.free: PASS
heap.kmalloc.nonzero: PASS
heap.kmalloc.distinct: PASS
heap.write: PASS
heap.free.restores: PASS
heap.kmalloc.large: PASS
heap.free.large: PASS
=== memTest: ALL PASS ===
```

- [ ] **Step 7: Write the changelog entry**

Create `CHANGELOG/05-26-05-27-buddy-heap.md`:

```markdown
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
```

- [ ] **Step 8: Commit**

```bash
git add x64barebones/Kernel/include/heap.h x64barebones/Kernel/memory/heap.c \
        x64barebones/Kernel/memory/memTest.c x64barebones/Kernel/kernel.c \
        CHANGELOG/05-26-05-27-buddy-heap.md
git commit -m "feat(mem): buddy-allocator kernel heap"
```

---

## Task 5: Integration — wire memory syscall, userland malloc/free, shell `memtest`

**Files:**
- Modify: `x64barebones/Kernel/systemCalls.c`
- Modify: `x64barebones/Userland/SampleCodeModule/systemCalls.c`
- Modify: `x64barebones/Userland/SampleCodeModule/include/stdlib.h`
- Modify: `x64barebones/Userland/SampleCodeModule/stdlib.c`
- Modify: `x64barebones/Userland/SampleCodeModule/shell.c`
- Modify: `x64barebones/Kernel/kernel.c`
- Create: `CHANGELOG/06-26-05-27-mem-integration.md`

- [ ] **Step 1: Replace the bump allocator in the kernel syscall**

Edit `x64barebones/Kernel/systemCalls.c`. Add include after line 4:

```c
#include <heap.h>
#include <memTest.h>
```

Add a memtest syscall number after `#define SYS_CALL_MEMORY 4` (line 9):

```c
#define SYS_CALL_MEMTEST 5
```

Delete the bump-allocator line (`static void * memory = (void *)0x900000;`, line 17).

Replace `memoryManagement` (lines 60-69) with a 3-argument version using the heap:

```c
uint64_t memoryManagement(uint64_t fnCode, uint64_t ptr, uint64_t nBytes){
	if(fnCode == MEMORY_ASIGN_CODE){          // allocate
		return (uint64_t) kmalloc(nBytes);
	}else if(fnCode == MEMORY_FREE_CODE){     // free
		kfree((void *) ptr);
		return 0;
	}
	return -1;
}
```

Update the dispatcher in `systemCall` (the `SYS_CALL_MEMORY` branch, lines 79-80) to pass `buf` as the pointer, and add the memtest branch:

```c
	}else if(systemCallNumber == SYS_CALL_MEMORY){
		return memoryManagement(fileDescriptor, (uint64_t) buf, nBytes);
	}else if(systemCallNumber == SYS_CALL_MEMTEST){
		memTest();
		return 0;
	}
```

- [ ] **Step 2: Thread the pointer through the userland syscall wrapper**

Edit `x64barebones/Userland/SampleCodeModule/systemCalls.c`. Add the memtest number after line 5:

```c
#define SYS_CALL_MEMTEST 5
```

Replace `memoryManagement` (lines 19-21) with a 3-argument version, and add a `memTest` wrapper:

```c
void * memoryManagement(int memoryCode, void * ptr, unsigned int nbytes){
	return (void *) systemCall(SYS_CALL_MEMORY, memoryCode, ptr, nbytes);
}

void memTest(){
	systemCall(SYS_CALL_MEMTEST, 0, 0, 0);
}
```

- [ ] **Step 3: Update the userland stdlib header**

Edit `x64barebones/Userland/SampleCodeModule/include/stdlib.h`. Find the `memoryManagement` declaration and replace it with the new signature; add `memTest`. (If `stdlib.h` has no such declaration, add both near the other prototypes.)

```c
void * memoryManagement(int memoryCode, void * ptr, unsigned int nbytes);
void   memTest();
```

- [ ] **Step 4: Fix userland malloc/free to use the new signature**

Edit `x64barebones/Userland/SampleCodeModule/stdlib.c`. Replace `malloc` (lines 4-6) and `free` (lines 27-29):

```c
void *malloc(size_t size){
	return memoryManagement(MEMORY_ASIGN_CODE, 0, size);
}
```

```c
void free(void *ptr){
	memoryManagement(MEMORY_FREE_CODE, ptr, 0);
}
```

- [ ] **Step 5: Add the `memtest` shell command**

Edit `x64barebones/Userland/SampleCodeModule/shell.c`. In `processComand`, add a branch before the final `else` (line 31). It runs the kernel self-test (results go to serial / QEMU stdout):

```c
	else if(!strcmp("memtest",buffer)){
		memTest();
		puts("  memtest eseguito (output su seriale)");
	}
```

Add the help line inside the `help` branch (after the `clear` line, line 14):

```c
		printf("  memtest : esegue i test del gestore memoria (output seriale)\n");
```

Ensure `stdlib.h` is included in `shell.c` (it already includes `<stdlib.h>` at line 3).

- [ ] **Step 6: Turn off the boot-time self-test**

Edit `x64barebones/Kernel/kernel.c`. Change the toggle added in Task 1:

```c
#define MEM_TEST_ON_BOOT 0   /* set to 0 for release builds */
```

This removes the boot-time `memTest()` + `isa-debug-exit` so the OS boots into the shell normally. The self-test is now reachable via the `memtest` shell command.

- [ ] **Step 7: Build and run interactively**

Run:
```bash
cd x64barebones && make all && ./run.sh
```
In the QEMU window, at the `$>` prompt type `help` and confirm `memtest` is listed. Type `memtest`; the kernel runs the self-test. To see PASS/FAIL lines, run instead with serial visible:
```bash
cd x64barebones && qemu-system-x86_64 -hda Image/x64BareBonesImage.qcow2 -m 512 \
	-serial stdio \
	-device rtl8139,netdev=n0,mac=DE:00:40:AA:21:2E -netdev user,id=n0
```
then type `memtest` — the terminal shows `=== memTest: ALL PASS ===`. Also verify `2048game` and other commands still work (regression: malloc still functions).

- [ ] **Step 8: Write the changelog entry**

Create `CHANGELOG/06-26-05-27-mem-integration.md`:

```markdown
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
```

- [ ] **Step 9: Commit**

```bash
git add x64barebones/Kernel/systemCalls.c \
        x64barebones/Userland/SampleCodeModule/systemCalls.c \
        x64barebones/Userland/SampleCodeModule/include/stdlib.h \
        x64barebones/Userland/SampleCodeModule/stdlib.c \
        x64barebones/Userland/SampleCodeModule/shell.c \
        x64barebones/Kernel/kernel.c \
        CHANGELOG/06-26-05-27-mem-integration.md
git commit -m "feat(mem): wire memory syscall to heap, fix malloc/free, add memtest command"
```

---

## Notes for the implementer

- **Toolchain:** the build uses Linux `gcc`/`ld`/`nasm` and `qemu-system-x86_64`. On Windows, build and run inside WSL or an equivalent Linux environment. All commands above are bash.
- **No standard library:** code is `-ffreestanding -nostdlib`. Do not include `<stdlib.h>`/`<string.h>` system headers in the kernel; use the project's own headers.
- **Why link-error red:** tests live in the same kernel binary, so a missing implementation shows up as a linker `undefined reference`. That is the intended red state before each implementation step.
- **Changelog rule (CLAUDE.md):** every task writes a `CHANGELOG/NN-...md` entry; numbering continues from `01` (this plan). Confirm the highest existing number before adding.
