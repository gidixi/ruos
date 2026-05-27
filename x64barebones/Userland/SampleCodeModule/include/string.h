#ifndef _STRING_
#define _STRING_


/*  Sorts de string str in reverse and returns it */
char* strrev(char* str);

/* Appends the first n characters of the string pointed to, by str2 to the end of the string pointed to by str1 */
char* strncat(char* str1, char* str2, int n);

/* Finds the index of the first appearance of the character s in the string str1 */
int strpbrk(char* str1, char s);

/* Appends the string pointed to, by str2 to the end of the string pointed to by str1 */
char* strcat(char* str1, char* str2);

/* Returns 1 if the string str1 contains the string str2 and 0 if it doesnt */
int strstr(char* str1, char* str2);

/* Computes the length of the string str up to but not including the terminating null character */
int strlen(char* str);

/* Returns positive if string str1 is bigger than str2, 0 if they are equals and negative otherwise */
int strcmp(const char* str1, const char* str2);

/* Returns the number that the string number represents */
int stringToNum(char* number);


void strcpy(char *dest, char *src);


void strncpy(char *dest, char *src, int n);
#endif