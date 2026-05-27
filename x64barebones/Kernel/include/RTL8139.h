#ifndef _RTL8139_H_
#define _RTL8139_H_

#include <stdint.h>
#define NULL ((void *)0)
#define MAC_SIZE 6

typedef struct {
	uint8_t mac[MAC_SIZE];
	uint8_t msg[1000];
	uint16_t length;
	uint8_t type;
} ethMsg;

void initRTL();

void sendMsg(ethMsg msg);

int getMsg(ethMsg * msg);

#endif