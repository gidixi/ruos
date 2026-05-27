#include "systemCalls.h"
#define SYS_CALL_READ 1
#define SYS_CALL_WRITE 2
#define SYS_CALL_CLEAR_SCREEN 3
#define SYS_CALL_MEMORY 4

int read(int fildes, void *buf, unsigned int nbytes){
	return systemCall(SYS_CALL_READ, fildes, buf, nbytes);
}

int write(int fildes, void *buf, unsigned int nbytes){
	return systemCall(SYS_CALL_WRITE, fildes, buf, nbytes);
}

void clearScreen(){
	systemCall(SYS_CALL_CLEAR_SCREEN, 1, 0, 0);
}

void * memoryManagement(int memoryCode, unsigned int nbytes){
	return (void *) systemCall(SYS_CALL_MEMORY,memoryCode,0,nbytes);
}