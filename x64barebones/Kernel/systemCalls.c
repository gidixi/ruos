#include "systemCalls.h"
#include <videoDriver.h>
#include <keyBoardDriver.h>
#include <RTL8139.h>

#define SYS_CALL_READ 1
#define SYS_CALL_WRITE 2
#define SYS_CALL_CLEAR_SCREEN 3
#define SYS_CALL_MEMORY 4

#define MEMORY_ASIGN_CODE 0
#define MEMORY_FREE_CODE 1

#define STANDARD_IO_FD 1
#define ETHERNET_FD 2

static void * memory = (void *)0x900000;

uint64_t clearScreenSys(){
	clearScreen();
	return 0;
}

uint64_t read(uint64_t fileDescriptor, void * buf, uint64_t nBytes){

	if(fileDescriptor == STANDARD_IO_FD){
		char * myBuf = (char *) buf;
		int cont = 1, readBytes=0;
		for (int i = 0; i < nBytes && cont; i++){
			*(myBuf + i) = (char) getKey();
			if(*(myBuf + i) == 0){
				cont = 0;
			}else{
				readBytes++;
			}
		}
		return readBytes;
	}else if(fileDescriptor == ETHERNET_FD){
		return getMsg((ethMsg *) buf);
	}
	return -1;
}

uint64_t write(uint64_t fileDescriptor, void * buf, uint64_t nBytes){

	if(fileDescriptor == STANDARD_IO_FD){
		char * myBuf = (char *) buf;
		int i;
		for(i = 0; i < nBytes && myBuf[i] != 0; i++){
			printCharacters(myBuf[i]);
		}
		return i;
	}else if(fileDescriptor == ETHERNET_FD){
		sendMsg(*((ethMsg *) buf));
		return nBytes;
	}
	return -1;
}

uint64_t memoryManagement(uint64_t fnCode, uint64_t nBytes){
	if(fnCode == MEMORY_ASIGN_CODE){//asigno memoria
		void * ptr = memory;
		memory += nBytes;
		return (uint64_t) ptr;
	}else if(fnCode == MEMORY_FREE_CODE){//libero memoria
		return 0;
	}
	return -1;
}


uint64_t systemCall(uint64_t systemCallNumber, uint64_t fileDescriptor, void * buf, uint64_t nBytes){
	if(systemCallNumber == SYS_CALL_READ){
		return read(fileDescriptor, buf, nBytes);
	}else if(systemCallNumber == SYS_CALL_WRITE){
		return write(fileDescriptor, buf, nBytes);
	}else if(systemCallNumber == SYS_CALL_CLEAR_SCREEN){
		return clearScreenSys();
	}else if(systemCallNumber == SYS_CALL_MEMORY){
		return memoryManagement(fileDescriptor, nBytes);
	}
	return 0;
}

