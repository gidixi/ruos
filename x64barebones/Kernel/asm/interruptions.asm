GLOBAL sti
GLOBAL setPicMaster
GLOBAL setPicSlave
GLOBAL irq0Handler
GLOBAL irq1Handler
GLOBAL irq11Handler
GLOBAL sysCallHandler

EXTERN irqDispatcher
EXTERN systemCall

%include "./asm/macros.m"

section .text

irq0Handler:
	irqHandler 0
irq1Handler:
	irqHandler 1
irq11Handler:
	irqHandlerSlave 11
	


sysCallHandler:
	pushaq

	;recivo parametros en un orden y es el mismo orden de salida
	call systemCall
	
	mov [return], rax ; guardo el valor de retorno

	popaq

	mov rax, [return]

	iretq



sti:
	sti
	ret


	
setPicMaster:
	push rbp
	mov rbp, rsp
	
	mov rax, rdi
	out 21h, al
	
	mov rsp, rbp
	pop rbp
	ret


setPicSlave:
	push rbp
	mov rbp, rsp
	
	mov rax, rdi
	out 0A1h, al
	
	mov rsp, rbp
	pop rbp
	ret


section .bss
	return resq 1