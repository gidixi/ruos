#ifndef _FRAME_ALLOCATOR_H
#define _FRAME_ALLOCATOR_H

#include <stdint.h>

void     initFrameAllocator(void);
uint64_t allocFrame(void);              /* physical addr of a free 4 KiB frame, or 0 */
uint64_t allocFrames(uint64_t n);       /* physical addr of n contiguous frames, or 0 */
void     freeFrame(uint64_t physAddr);
uint64_t freeFrameCount(void);          /* number of free frames (for tests) */

#endif
