#include "RTL8139.h"
#include <stdint.h>
#include <sysIO.h>

#include <naiveConsole.h>

#define IO_ADDRESS 0xC000 // Get from pci search

#define CONFIG_1 0x52	// Configuration Register 1 Address
#define ISR 0x3E 	// Interrupt Status Register Start Address
#define CR 0x37		// Command Register Address
#define IMR 0x3C 	// Interrupt Mask Register 
#define RCR 0x44 	// Receive (Rx) Configuration Register 

#define RBSTART 0x30 	// Receive Buffer Start Address 
#define TSAD0 0x20		// Transmit Start Address of Descriptor0
#define TSAD1 0x24		// Transmit Start Address of Descriptor1
#define TSAD2 0x28		// Transmit Start Address of Descriptor2
#define TSAD3 0x2C		// Transmit Start Address of Descriptor3

#define TSD0 0x10 		// Transmit Status of Descriptor 0
#define TSD1 0x14 		// Transmit Status of Descriptor 1
#define TSD2 0x18 		// Transmit Status of Descriptor 2
#define TSD3 0x1C 		// Transmit Status of Descriptor 3

#define CHECK_TOK 0x4		// Packet transmision completed succesfully
#define CHECK_ROK 0x1		// Succesfull completition of packet reception

#define CLEAR_TOK 0xFB 		// Clearing the Transmit OK bit
#define CLEAR_ROK 0xFE		// Clearing the Recive OK bit
#define CLEAR_OWN 0xFFF		// Clearing the OWN bit

#define TR_BUFFER_SIZE 8*(1024) 			// Osdev suggested size for the transmit buffer
#define RE_BUFFER_SIZE 8*(1024)+16+1500 	// Osdev suggested size for the recive buffer	

#define MESSAGE_BUFFER_SIZE 100		// Internal circular buffer size
#define PAYLOAD_SIZE 1000

#define BROADCAST 0xFF 		// Broadcast MAC value

static char trBuffer[3][TR_BUFFER_SIZE] = {{0}};
static char reBuffer[RE_BUFFER_SIZE] = {0};


typedef struct {
	uint8_t macDest[MAC_SIZE];
	uint8_t macSrc[MAC_SIZE];
	uint16_t length;
	uint8_t payload[PAYLOAD_SIZE];//CAMBIAR
	uint32_t frameCheck;
} ethFrame;

static ethFrame messages[MESSAGE_BUFFER_SIZE] = {{{0}}};
static int nexToWrite=0;
static int nextToRead=0;

void mymemcpy( void * dest, void * src, int length){
	char * d = dest;
	char * s = src;

	for(int i = 0; i < length; i++){
		d[i] = s[i];
	}
}

void transmit(ethFrame * frame){
	if(frame == NULL){
		return;
	}

	int length = 0;

	/* Copy ethernet frame to transmit buffer */
	mymemcpy(trBuffer[0]+length,frame->macDest,MAC_SIZE);
	length += MAC_SIZE;
	mymemcpy(trBuffer[0]+length,frame->macSrc,MAC_SIZE);
	length += MAC_SIZE;
	trBuffer[0][length] = frame->length;
	length += 2;
	mymemcpy(trBuffer[0]+length,frame->payload,frame->length);
	length += frame->length;
	trBuffer[0][length] = frame->frameCheck;
	length += 2;

	sysOutLong( IO_ADDRESS + TSD0, length & CLEAR_OWN);//clear own bit

}

void getMacAdress(uint8_t macDest[MAC_SIZE]){
	uint32_t mac=sysInLong(IO_ADDRESS);
	macDest[0]=mac;
	macDest[1]=mac>>8;
	macDest[2]=mac>>16;
	macDest[3]=mac>>24;
	mac=sysInLong(IO_ADDRESS+4);
	macDest[4]=mac;
	macDest[5]=mac>>8;
}

void sendMsg(ethMsg message){
	ethFrame frame;
	for(int i=0; i<MAC_SIZE; i++){
		frame.macDest[i]=message.mac[i];
	}

	getMacAdress(frame.macSrc);
	frame.length=message.length+1;
	frame.payload[0]=message.type;
	mymemcpy(frame.payload+1,message.msg,message.length);
	frame.frameCheck=message.type;

	transmit(&frame);
}

int getMsg(ethMsg * msg){
	
	if(nextToRead == nexToWrite){//estoy en el final del buffer, no tengo mensajes
		return 0;
	}

	//pongo el mensaje en msg
	mymemcpy(msg->mac,messages[nextToRead].macSrc,MAC_SIZE);
	msg->length=messages[nextToRead].length-1;
	msg->type=messages[nextToRead].payload[0];
	mymemcpy(msg->msg,messages[nextToRead].payload+1,msg->length);
	nextToRead++;

	nextToRead %= MESSAGE_BUFFER_SIZE;

	return msg->length + MAC_SIZE + sizeof(msg->length);

}

int isForMe( ethFrame * frame ){
	uint8_t myMAC[MAC_SIZE];
	getMacAdress(myMAC);
	int cont = 1;
	for(int i = 0; i < MAC_SIZE && cont; i++){
		if(frame->macDest[i] != myMAC[i]){
			cont = 0;
		}
	}

	if(!cont){
		cont = 1;
		for(int i = 0; i < MAC_SIZE && cont; i++){
			if( frame->macDest[i] != BROADCAST){
				cont = 0;
			}
		}
	}
	return cont;
}


void rtlHandler(){
	uint16_t status = sysInWord( IO_ADDRESS + ISR);

	if( (status & CHECK_ROK) != 0 ){ // ISR bit 0 enabled indicates Recive OK

		//ncPrint("-----Message recieved---");ncNewline();
		ethFrame * frame= (ethFrame *) (reBuffer+4);

		if( !messages[nexToWrite].macDest[0] && isForMe(frame) ){
			mymemcpy(messages+(nexToWrite++),frame,sizeof(ethFrame));
			//printFrame(messages+(nextToRead++));
		}else{
			//buffer lleno o no es para mi
			//ncPrint("BUFFER LLENO o no es para mi");
		}
		//verifico si llegue al final del buffer de mensajes
		nexToWrite %= MESSAGE_BUFFER_SIZE;
		//pongo el ROK en 0
		sysOutWord( IO_ADDRESS + ISR, status & CLEAR_ROK);

	}else if( (status & CHECK_TOK) != 0 ){ // ISR bit 2 enabled indicates Transmit OK

	//ncPrint("------Message sent----");ncNewline();ncNewline();

		//pongo el TOK en 0
		sysOutWord( IO_ADDRESS + ISR, status & CLEAR_TOK);

	}
	initRTL();
}

void initRTL(){
	
	/* Send 0x00 to the CONFIG_1 register. This should essentially "power on" the device. */
	sysOutByte(IO_ADDRESS + CONFIG_1, 0x0);

	/* Software reset to clear the RX and TX buffers and set everything back to defaults. */
	sysOutByte(IO_ADDRESS + CR, 0x10);
	/* Checking the RST bit to make sure that the chip has finished the reset. */
	while( (sysInLong(IO_ADDRESS + CR) & 0x10) != 0 );

	/* Setting the Read and Write buffers memory location to the RTL */
	sysOutLong(IO_ADDRESS + RBSTART, (uintptr_t)reBuffer);
	sysOutLong(IO_ADDRESS + TSAD0, (uintptr_t)trBuffer[0]);
	sysOutLong(IO_ADDRESS + TSAD1, (uintptr_t)trBuffer[1]);
	sysOutLong(IO_ADDRESS + TSAD2, (uintptr_t)trBuffer[2]);
	sysOutLong(IO_ADDRESS + TSAD3, (uintptr_t)trBuffer[3]);

	/* Set the RTL8139 to accept only the Transmit OK (TOK) and Receive OK (ROK) interrupts
	** we would have the TOK and ROK bits of the IMR high and leave the rest low. */
	sysOutWord(IO_ADDRESS + IMR, 0x0005);

	/* Telling the RTL8139 to accept AB+AM+APM+AAP packets (0xf).
	** Setting WRAP bit, so the packet will be written contiguously in memory. */
	sysOutLong( IO_ADDRESS + 0x44, 0xf | (1 << 7) );

	/* Start up the RX and TX functions.
	** Once this is completed, then the card will start allowing packets in and/or out. */
	sysOutByte( IO_ADDRESS + CR, 0x0C);

}



/* DEBUGGING */

void printFrame(ethFrame * frame){
	ncNewline();ncPrint("Mac destination: ");
		for(int i=0; i<MAC_SIZE; i++){
			ncPrintHex(frame->macDest[i]);
		}
		ncNewline();ncPrint("Mac Source: ");
		for(int i=0; i<MAC_SIZE; i++){
			ncPrintHex(frame->macSrc[i]);
		}
		ncNewline();ncPrint("length: ");
		ncPrintDec(frame->length);ncNewline();
		ncPrint("Message: ");
		for(int i=0; i<frame->length; i++){
			ncPrintChar(frame->payload[i]);
		}
		ncNewline();
		ncNewline();ncPrint("frameCheck: ");
		ncPrintDec(frame->frameCheck);ncNewline();
		ncPrint("---------End of message----");ncNewline();ncNewline();
}