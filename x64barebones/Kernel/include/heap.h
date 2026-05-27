#ifndef _HEAP_H
#define _HEAP_H

#include <stdint.h>

void     initHeap(void);
void *   kmalloc(uint64_t size);
void     kfree(void * ptr);
uint64_t heapFreeBytes(void);   /* total free bytes across the buddy lists (for tests) */

#endif
