#include <memTest.h>
#include <sysIO.h>
#include <stdint.h>
#include <frameAllocator.h>
#include <paging.h>
#include <heap.h>

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

void memTest(void) {
	failCount = 0;
	serialInit();
	serialPrint("=== memTest start ===\n");
	testFrameAllocator();
	testPaging();
	testHeap();
	serialPrint(failCount == 0 ? "=== memTest: ALL PASS ===\n"
	                           : "=== memTest: FAILURES ===\n");
}
