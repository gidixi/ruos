#include <paging.h>
#include <frameAllocator.h>
#include <stdint.h>

#define ENTRY_ADDR_MASK 0x000FFFFFFFFFF000ULL
#define PAGE_PS 0x80   /* bit 7: 2 MiB/1 GiB large page when set on a PDPT/PD entry */

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

/* Returns the next-level table pointer; allocates+zeroes it when create != 0.
 * LIMITATION: assumes the walked path uses only 4 KiB pages. If an entry has the
 * PS bit set (a 2 MiB/1 GiB large page, as Pure64 uses for the low identity map),
 * this returns 0 rather than misreading the large-page frame as a table pointer.
 * So do NOT map over the Pure64 low identity map; use fresh virtual ranges. */
static uint64_t * getOrCreate(uint64_t * table, uint64_t idx, int create) {
	if (!(table[idx] & PAGE_PRESENT)) {
		if (!create) return 0;
		uint64_t frame = allocFrame();    /* < 4 GiB -> identity-mapped, usable as ptr */
		if (!frame) return 0;
		uint64_t * t = (uint64_t *) frame;
		for (int i = 0; i < 512; i++) t[i] = 0;
		/* Intermediate tables are created permissive (RW|USER); effective
		 * permission is the AND across levels, gated by the final PTE. */
		table[idx] = frame | PAGE_PRESENT | PAGE_RW | PAGE_USER;
		return (uint64_t *)(table[idx] & ENTRY_ADDR_MASK);
	}
	if (table[idx] & PAGE_PS) return 0;   /* large-page leaf: not a table pointer */
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
	/* TODO: copies all 512 PML4 entries (user + higher half). Fine while there is
	 * a single shared address space; real process isolation should share only the
	 * kernel/higher-half entries. */
	for (int i = 0; i < 512; i++) newPml4[i] = cur[i];   /* share kernel mappings */
	return newPml4;
}

void switchAddressSpace(uint64_t * pml4) {
	__asm__ volatile("mov %0, %%cr3" :: "r"((uint64_t) pml4) : "memory");
}
