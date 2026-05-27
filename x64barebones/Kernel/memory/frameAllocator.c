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
