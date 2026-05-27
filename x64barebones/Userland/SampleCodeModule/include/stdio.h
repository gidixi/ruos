#ifndef STDIO
#define STDIO

#include <stdint.h>
int printString(char * str);

int putchar(char c);

int getchar();

void printf(const char* format, ...);

int puts(char * str);

void putNum(int num);

int ungetc(int c);

void scanf(const char *format, ...);

void getline(char* buffer);

int startsWith(const char * str1, const char * str2);

#endif