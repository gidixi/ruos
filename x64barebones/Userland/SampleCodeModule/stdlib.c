#include <stdlib.h>
#include <systemCalls.h>

void *malloc(size_t size){
	return memoryManagement(MEMORY_ASIGN_CODE, size);
}

void *realloc(void *ptr, size_t size){
	void * nPtr = malloc(size);
	char * cPtr = (char *) nPtr;
	for(int i = 0; i<size; i++){
		cPtr[i] = *((char *) ptr+i);
	}
	return nPtr;
}

void *calloc(size_t nitems, size_t size){
	char * initialize;
	void * ptr = malloc(nitems * size);
	for(int i = 0; i<nitems * size; i++){
		initialize = (char *) ptr+i;
		*initialize = 0;
	}
	return ptr;
}

void free(void *ptr){
	memoryManagement(MEMORY_FREE_CODE,NULL);
}

int abs(int x){
	if(x<0){
		x*=-1;
	}
	return x;
}