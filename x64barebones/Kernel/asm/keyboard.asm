GLOBAL checkKeyboard
GLOBAL getKeyboard

checkKeyboard:
	push rbp
	mov rbp,rsp

	in al, 64h
	and al, 1h

	mov rsp,rbp
	pop rbp
	ret

getKeyboard:
	push rbp
	mov rbp,rsp

	in al, 60h


	mov rsp,rbp
	pop rbp
	ret