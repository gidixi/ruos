#ifndef _SYSTEM_CALLS_USER_
#define _SYSTEM_CALLS_USER_

#include <stdint.h>

#define MEMORY_ASIGN_CODE 0
#define MEMORY_FREE_CODE 1

#define STANDARD_IO_FD 1
#define ETHERNET_FD 2

uint64_t systemCall(uint64_t systemCallNumber, uint64_t fileDescriptor, void * buf, uint64_t nBytes); // asm function


/* Returns the number of bytes that were read. If value is negative, then the system call returned an error. */
int read(int fildes, void *buf, unsigned int nbytes);


/* Returns the number of bytes that were written. If value is negative, then the system call returned an error. */
int write(int fildes, void *buf, unsigned int nbytes);


void clearScreen();

void *memoryManagement(int memoryCode, unsigned int nbytes);

#endif