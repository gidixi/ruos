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
		if (!listRemove(order, buddy)) break;  /* buddy not free -> stop merging */
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
