#include <systemCalls.h>
#include <stdio.h>
#include <string.h>
void shell();

/* sampleCodeModule.c */

static int var1 = 0;
static int var2 = 0;

int main() {
	shell();
	
	//Test if BSS is properly set up
	if (var1 == 0 && var2 == 0)
		return 0xDEADC0DE;

	return 0xDEADBEEF;
}