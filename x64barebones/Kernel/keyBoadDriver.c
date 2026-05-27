#include "keyBoardDriver.h"
#define BUFFER_SIZE 100
#define L_SHIFT_PRESSED 0x2A
#define L_SHIFT_RELEASED 0xAA
#define R_SHIFT_PRESSED 0x36
#define R_SHIFT_RELEASED 0xB6
#define CAPS_LOCK_PRESSED 0x3A
#define CAPS_LOCK_RELEASED 0xBA

unsigned int checkKeyboard();
int getKeyboard();

static char lKeyboard[128] = {0,'`','1','2','3','4','5','6','7','8','9','0','-','=','\b'/*borrar*/,
						'\t'/*tab*/,'q','w','e','r','t','y','u','i','o','p','[',']','\n',
						0/*MAYUS*/,'a','s','d','f','g','h','j','k','l',';','\'','\\',
						0/*shift*/,0,'z','x','c','v','b','n','m',',','.','/',0/*shift*/,0,0,' '};
	

static char uKeyboard[128] = {0,'~','!','@','#','$','%','^','&','*','(',')','_','+','\b'/*borrar*/,
						'\t'/*tab*/,'Q','W','E','R','T','Y','U','I','O','P','{','}','\n',
						0/*MAYUS*/,'A','S','D','F','G','H','J','K','L',':','"','|',
						0/*shift*/,0,'Z','X','C','V','B','N','M','<','>','?',0/*shift*/,0,0,' '};

static int buffer[BUFFER_SIZE] = {0};
static int * current = buffer;
static int * last = buffer;

int uppercase = 1;

void keyBoardHandler(){
	if(checkKeyboard()){
		int character;
		int key = getKeyboard();

		//veo si aprete para usar mayusculas
		if(key == L_SHIFT_PRESSED || key == R_SHIFT_PRESSED){
			uppercase *=-1;
		}else if(key == L_SHIFT_RELEASED || key == R_SHIFT_RELEASED){
			uppercase *=-1;
		}else if(key == CAPS_LOCK_PRESSED ){
			uppercase *=-1;
		}

		if(key >= 128){ // no es un caracter valido
			return;
		}


		if(uppercase==-1){
			character = uKeyboard[key];
		}else{
			character = lKeyboard[key];
		}

		if(*last == 0 && character != 0){
			*(last++) = character;
			if(last - buffer >= BUFFER_SIZE){ //vuelvo al principio del buffer
				last = buffer;
			}
		}else{
			//overflow buffer exception
		}
	
	}
}

int getKey(){
	int character = 0;

	if(*current == 0){//si tengo el buffer vacio no avanzo el puntero
		return 0;
	}

	character = *current;
	*(current++) = 0;
	if(current - buffer >= BUFFER_SIZE){ //vuelvo al principio del buffer
		current = buffer;
	}
		
	return character;
}
