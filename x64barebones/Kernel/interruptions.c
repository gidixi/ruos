#include <interruptions.h>
#include <keyBoardDriver.h>
#include <systemCalls.h>

#include <naiveConsole.h>

void sti();
unsigned char getKeyboard();
void printCharacter();
void irq0Handler();
void irq1Handler();
void irq11Handler();
void sysCallHandler();
void setPicMaster(uint16_t);
void setPicSlave(uint16_t);
void rtlHandler();


#pragma pack(push)
#pragma pack(1)


typedef struct { 
	uint16_t offset_l; //bit 0..15
	uint16_t selector;
	uint8_t zero_l;
	uint8_t attrs;
	uint16_t offset_m; //16..31
	uint32_t offset_h; //32..63
	uint32_t zero_h;
} IDTEntry_t;

#pragma pack(pop)

static IDTEntry_t* IDT = (IDTEntry_t*) 0x0; // pongo la idt en la posicion 0 de memeoria

void tickHandler() {

}

typedef void (*handler_t)(void);

handler_t handlers[] = {tickHandler,keyBoardHandler,0,0,0,0,0,0,0,0,0,rtlHandler};

void irqDispatcher(int irq) {
	handlers[irq]();
}

void iSetHandler(int index, uint64_t handler) {
	IDT[index].offset_l = (uint16_t) handler & 0xFFFF;
	IDT[index].offset_m = (uint16_t) (handler >> 16) & 0xFFFF;
	IDT[index].offset_h = (uint32_t) (handler >> 32) & 0xFFFFFFFF;
	
	IDT[index].selector = 0x08;
	IDT[index].zero_l = 0;
	
	IDT[index].attrs = 0x8E;
	IDT[index].zero_h = 0;	
	
}

void initializeInterruptions(){
	iSetHandler(0x20, (uint64_t) irq0Handler);
	iSetHandler(0x21, (uint64_t) irq1Handler);
	iSetHandler(0x2B, (uint64_t) irq11Handler);

	iSetHandler(0x80, (uint64_t) sysCallHandler); //software interruptions, systemcall

	setPicMaster(0xF8); // activo las irq0(timer tick), irq1(teclado) y la irq2(slave pic) en el pic master
	setPicSlave(0xF7); // activo la irq11(RTL8139) que esta en slave pic
	
	sti();
}

