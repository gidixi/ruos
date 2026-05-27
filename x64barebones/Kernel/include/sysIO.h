#ifndef _SYS_IO_
#define _SYS_IO_

#include <stdint.h>

extern void sysOutByte(uint32_t address, uint8_t value);
extern void sysOutWord(uint32_t address, uint16_t value);
extern void sysOutLong(uint32_t address, uint32_t value);
extern uint8_t sysInByte(uint32_t address);
extern uint16_t sysInWord(uint32_t address);
extern uint32_t sysInLong(uint32_t address);

#endif