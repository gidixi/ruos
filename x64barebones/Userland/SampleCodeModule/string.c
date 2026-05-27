#include <string.h>
#include <stdio.h>

int strlen(char* str){
	int length=0;
	while(*str){
		length++;
		str++;
	}
	return length;
}


// invierte el orden de los caracteres de la palabra
char* strrev(char* str){
	int medio = strlen(str)/2;
	int i = 0;
	while(i<medio){
		char aux = str[i];
		str[i] = str[2*medio-i];
		str[2*medio-i] = aux;
		i++;
	}
	return str;	
}

// concatena un string a los primeros n caracteres de otra
char * strncat(char* str1, char* str2, int n){
	int length = strlen(str1);
	int i = 0;
	while(str2[i] && i < n){
		str1[length] = str2[i];
		length++;
		i++;
	}
	str1[length] = 0;
	return str1;
}

// devuelve el indice de la primera ocurrencia de s en el string str1
int strpbrk(char* str1, char s){
	int i = 0;
	while(str1[i]){
		if(str1[i] == s){
			return i;
		}
		i++;
	}
	return -1;
}

// concatena un string a otro
char * strcat(char* str1, char* str2){
	int length = strlen(str1);
	while(*str2){
		str1[length] = *str2;
		length++;
		str2++;
	}
	str1[length] = 0;
	return str1;
}


void strcpy(char *dest, char *src){
	while(*src){
		*dest = *src;
		dest++;
		src++;
	}
	*dest =0;
	return;
}


void strncpy(char *dest, char *src, int n){
	while(*src && n){
		*dest = *src;
		dest++;
		src++;
		n--;
	}
	*dest =0;
	return;

}

int strcmp(const char* str1, const char* str2){
	while((*str1 == *str2) && *str1 != 0 && *str2 != 0){
		str1++;
		str2++;
	}
	return *str1 - *str2;
}


// devuelve 1 si el string2 esta contenido en el string1 y 0 sino
int strstr(char* str1, char* str2){
	int count=0;
	int flag = 0;
	while(*str1){
		while(*str1 == *str2 && *str2 && *str1){
			str1++;
			str2++;
			count++;
			flag = 1;
		}
		if(!*str2){
			return 1;
		}else if(flag){
			str2-=count;
		}
		if(!*str1){
			return 0;
		}
		flag = 0;
		count=0;
		str1++;
	}
	return 0;
}

// devuelve el numero que representa el string number
int stringToNum(char* number){
	int ret = 0;
	int i = 0;
	while(*number){
		ret = ret*10 + *number -'0';
		number++;
		i++;
	}
	return ret;
}
