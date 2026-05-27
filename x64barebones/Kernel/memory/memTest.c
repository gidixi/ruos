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
