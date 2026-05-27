#include <stdint.h>
#define WINDOW_WIDTH 80
#define WINDOW_HEIGHT 25
#define DEFAULT_COLOR 0x0F
#define END_OF_SCREEN WINDOW_WIDTH*2 * WINDOW_HEIGHT

static char * screen = (char *)0xB8000;
static char * currentPosition = (char*)0xB8000;

void clearScreen(){
	for(int i = 0; i < WINDOW_WIDTH*2 * WINDOW_HEIGHT; i++){
		if(i%2 == 1){
			screen[i] = DEFAULT_COLOR;
		}else{
			screen[i] = ' ';
		}
	}
	currentPosition = screen;
}

void moveScreenUp(){
	for(int i = 0; i< WINDOW_WIDTH*2 * (WINDOW_HEIGHT-1); i++){
		screen[i] = screen[i+WINDOW_WIDTH*2];
	}
	for(int i = 0; i < WINDOW_WIDTH*2; i+=2){
		*(screen + END_OF_SCREEN - WINDOW_WIDTH*2 + i) = ' ';
		*(screen + END_OF_SCREEN - WINDOW_WIDTH*2 + i + 1) = DEFAULT_COLOR;
	}
	currentPosition = screen + END_OF_SCREEN - WINDOW_WIDTH*2;
}

void checkEndOfScreen(){
	if(currentPosition >= screen + END_OF_SCREEN){
		moveScreenUp();
	}
}

void print(const char * string){
	while(*string){
		*(currentPosition++) = *(string++);
		*(currentPosition++) = DEFAULT_COLOR;
		checkEndOfScreen();
	}
}

void newLine(){
	currentPosition +=  (screen - currentPosition) % (WINDOW_WIDTH*2) + WINDOW_WIDTH*2;
	checkEndOfScreen();
}

void backSpace(){
	if(currentPosition == screen){
		return;
	}
	currentPosition-=2;
	*currentPosition = ' ';
}

void printTab(){
	currentPosition += 8; 
}

void printCharacters(const char character){
	//analizo caracteres especiales
	if(character == '\n'){
		newLine();
	}else if(character == '\b'){
		backSpace();
	}else if(character == '\t'){
		printTab();
	}else{
		*(currentPosition++) = character;
		*(currentPosition++) = DEFAULT_COLOR;
	}
	checkEndOfScreen();
}

