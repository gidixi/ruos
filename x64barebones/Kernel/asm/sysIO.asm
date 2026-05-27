GLOBAL sysOutByte
GLOBAL sysOutWord
GLOBAL sysOutLong
GLOBAL sysInByte
GLOBAL sysInWord
GLOBAL sysInLong

section .text

sysOutByte:
	push rdx
	mov edx, edi
	mov eax, esi

	out dx, al

	pop rdx
	ret

sysOutWord:
	push rdx
	mov edx, edi
	mov eax, esi

	out dx, ax

	pop rdx
	ret

sysOutLong:
	push rdx
	mov edx, edi
	mov eax, esi

	out dx, eax

	pop rdx
	ret


sysInByte:
	push rdx

	mov edx, edi

	in al, dx

	pop rdx
	ret

sysInWord:
	push rdx

	mov edx, edi

	in ax, dx

	pop rdx
	ret

sysInLong:
	push rdx

	mov edx, edi

	in eax, dx

	pop rdx
	ret