#include <myChat.h>
#include <stdio.h>
#include <string.h>
#include <systemCalls.h>

#define MAX_USERS 50
#define USERNAME_MAXLENGTH 50
#define MAC_SIZE 6
#define ONLINE 0
#define OFFLINE 1
#define MESSAGE 2
#define MSG_MAXLENGTH 1000


typedef struct {
	uint8_t mac[MAC_SIZE];
	char msg[MSG_MAXLENGTH];
	uint16_t length;
	uint8_t type;
} ethMsg;

void mymemcpy( void * dest, void * src, int length);
void send(int type, uint8_t * dest , char * message);
uint8_t * getMac(char user[USERNAME_MAXLENGTH]);
void showOnlineUsers();
void maccpy(uint8_t mdest[MAC_SIZE],uint8_t msrc[MAC_SIZE]);
void offlineUser(char user[USERNAME_MAXLENGTH]);
int command(char * str);
int isPrivate(char *str);
void deleteLine(int amount);
void welcome();
void save(char * user, uint8_t mac[MAC_SIZE]);
int cmpMac(uint8_t mac1[MAC_SIZE],uint8_t mac2[MAC_SIZE]);
char * getName(uint8_t mac[MAC_SIZE]);
void recieve(ethMsg msg);
void myChat();
void printMac(uint8_t  * mac);
void help();
void validName(char * name);



static uint8_t public[MAC_SIZE] ={0xFF,0xFF,0xFF,0xFF,0xFF,0xFF};
static char user[USERNAME_MAXLENGTH];
static char userList[USERNAME_MAXLENGTH][MAX_USERS];
static uint8_t macList[MAC_SIZE][MAX_USERS]={{0}};
static int usersCount=0;
static char generic[]="Your friend";


void mymemcpy( void * dest, void * src, int length){
	char * d = dest;
	char * s = src;

	for(int i = 0; i < length; i++){
		d[i] = s[i];
	}
}

void send(int type, uint8_t * dest , char * message){
	ethMsg msg={{0},{0},0};
	maccpy(msg.mac,dest);
	strcpy(msg.msg,message);
	msg.length=strlen(message);
	msg.type=type;
	msg.msg[msg.length]=0;
	write(2,&msg,msg.length);
}



uint8_t * getMac(char user[USERNAME_MAXLENGTH]){
	int i;
	for(i=0;i<usersCount;i++){
		if(!strcmp(userList[i],user)){
			return macList[i];
		}
	}
		return 0;
}

void printMac(uint8_t  * mac){
	for(int j=0; j<MAC_SIZE;j++){
			printf("%d\n",mac[j]&0xFF );
		}
}

void showOnlineUsers(){
	if(!usersCount){
		printf("\n\n          There are no users online\n");
		return;
	}
	printf("\n\nThis are the users online: \n");

	for (int i = 0; i < usersCount; ++i){
		printf("             %s\n",userList[i] );
	}
}

void maccpy(uint8_t mdest[MAC_SIZE],uint8_t msrc[MAC_SIZE]){
	for(int i = 0; i < MAC_SIZE ; i++){
		mdest[i] = msrc[i];
	}
	return;
}

void offlineUser(char user[USERNAME_MAXLENGTH]){
	int i = 0;
	int j;
	int flag = 1;
	while(i<usersCount && flag){
		if(!strcmp(userList[i],user)){
			j = i++;
			flag = 0;
		}
	}
	strcpy(userList[j],userList[usersCount-1]);
	maccpy(macList[j],macList[usersCount-1]);
	usersCount--;

}

int command(char * str){
	if(!strcmp("exit",str)){
		send(OFFLINE,public,user);
		printf("\n          Thanks for using out chat room! Bye bye!\n");
		return 1;
	}else if(str[0]==0){
		return 0;
	}else if(!strcmp("online",str)){
		showOnlineUsers();
	}else if(!strcmp("help",str)){
		help();
	}else if(isPrivate(str)){
		char mensaje[MSG_MAXLENGTH]={0};
		strcat(mensaje,"private - ");
		char name[USERNAME_MAXLENGTH];
		int i = 0;
		str++;
		while(str[i] !=' ' && str[i]!=0){
			name[i] = str[i];
			i++;
		}
		name[i] = 0;
		strcat(mensaje,str+i);
		send(MESSAGE,getMac(name),mensaje);
	}else if(isPrivate(str) == 2){
		//no mando el mensaje
	}else{
		send(MESSAGE,public,str);
	}
	return 0;

}

int isPrivate(char *str){
	char name[USERNAME_MAXLENGTH];
	int i = 0;
	if(str[0]!='@'){
		return 0;
	}
	str++;
	while(str[i] !=' ' && str[i]!=0){
		name[i] = str[i];
		i++;
	}
	name[i] = 0;
	if(getMac(name)==0){
		printf("\n\n        I'm sorry %s is not online\n", name);
		return 2;
	}
	return 1;
}



void deleteLine(int amount){
	while(amount){
		putchar('\b');
		amount--;
	}
}
void help(){
	printf("\n\nThis are the available commands:\n");
	printf("     help: show comands\n");
	printf("     exit: exit chat room\n");
	printf("     online: show users online\n");
	printf("     @(username): send private message to that user\n");
}


void welcome(){
	printf("        ***********************************************\n");
	printf("        **                                           **\n");
	printf("        **            Welcome to SkyChat!            **\n");
	printf("        **                                           **\n");
	printf("        ***********************************************\n");
	help();
	printf("\n" );

	char c;
	int j = 0;
	printf("%s", "What's your name?: ");
	while((c=getchar())!='\n' && j<USERNAME_MAXLENGTH){
		if(c == '\b'){
			if(j!=0 ){			
				putchar(c);
				j--;
			}
		}else{
		putchar(c);
		user[j++] = c;
		}
	}
	user[j] = 0;
	validName(user);
	putchar('\n');
	putchar('\n');
	putchar('\n');
	
}


void validName(char * name){
	int i = 0;
	int j = 0;
	while(name[i]){
		while(name[i] == ' '){
			i++;
		}
		name[j] = name[i];
		i++;
		j++;
	}
	name[j] = 0;
}


void save(char * user, uint8_t mac[MAC_SIZE]){
	strcpy(userList[usersCount],user);
	maccpy(macList[usersCount],mac);
	usersCount++;
}

int cmpMac(uint8_t mac1[MAC_SIZE],uint8_t mac2[MAC_SIZE]){
	for(int i = 0; i < MAC_SIZE; i++){
		if(mac1[i] != mac2[i]){
			return 0;
		}
	}
	return 1;
}

char * getName(uint8_t mac[MAC_SIZE]){
	int i;
	for(i=0; i<usersCount ;i++){
		if(cmpMac(mac,macList[i])){
			return userList[i];
		}
	}
	return generic;
}

void recieve(ethMsg msg){
	msg.msg[msg.length]=0;//convierto el mensja a string

	if(msg.type==ONLINE){
		printf("           %s is now online!\n",msg.msg );
		save(msg.msg,msg.mac);//gauradar MAC y nombre de usario
	}else if(msg.type== OFFLINE){
		printf("           %s is now offline\n",msg.msg );
		offlineUser(msg.msg);
	}else if(msg.type == MESSAGE){
		printf("%s: %s\n",getName(msg.mac), msg.msg);				//imprimir mensaje
	}else{
		printf("Oh Oh algo salio mal! msg.type: %d\n",msg.type);
	}
}



void myChat(){
	char buffer[200];
	int i=0;
	int exit=0;
	char c;
	int deleteFlag=0;
	

	welcome();
	clearScreen();
	send(ONLINE,public,user);

	
	
	while(!exit){
		if(!i && !deleteFlag){
			printf("\n%s: ",user);
		}
		//verifico si recibi algo
		int cFlag;
		int mFlag;
		ethMsg msg;
		/*Aprovecho que el read me devuelve la cantidad de bits que copio
		 si no hay nada para leer read devuelve cero.
		 Este while queda loopeando hasta que halla algo para leer*/
		while ( (cFlag=read(1,&c,1))==0 && (mFlag=read(2,&msg,100))==0 );
			
		if(cFlag){
			if(c == '\n'){
				if(i!=0){
					buffer[i]=0;
					exit=command(buffer);//mandar mensaje
					i=0;
					putchar(c);
				}else{
					deleteFlag=1;
				}
			}else if(c == '\b'){
				if(i!=0 ){			
					putchar(c);
					i--;
				}if(i ==0){
					deleteFlag=1;
				}
			}else{
				buffer[i++]=c;
				putchar(c);
				deleteFlag=0;
			}	

			
			cFlag=0;
		}
		if(mFlag){
			buffer[i]=0;
			deleteLine(i+2+strlen(user));
			recieve(msg);
			
			if(i){
				printf("%s: %s",user,buffer);				//volver a imprimir linea
			}						
			mFlag=0;
		}
	}

}
