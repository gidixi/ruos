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
