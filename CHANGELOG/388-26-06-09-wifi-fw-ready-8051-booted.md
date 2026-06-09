# 388 — WiFi RTL8188EU: `fw ready` — 8051 avviato, firmware in esecuzione (HW)

**Data:** 2026-06-09

## Cosa (VALIDATO su hardware reale)
Bring-up del firmware **completo**: `wifi: reset_8051 sysfunc=0xfc1c off_ok=true
on_ok=true` poi **`wifi: fw ready`** → l'8051 è partito e il firmware gira sul chip.
SP2 (firmware bring-up) **completo e validato su metallo**, senza freeze né replug.

## Root cause risolto (bug di 1 bit)
`reset_8051` toggleva il bit sbagliato di `REG_SYS_FUNC` (0x02):
- **prima:** `FEN_CPUEN = 0x04` = BIT2 = `SYS_FUNC_USBA` (enable PHY analogica USB) →
  cleararlo **uccideva la connessione USB** mid-transfer → `code=4` + pipe wedge.
- **fix:** `SYS_FUNC_CPU_EN = 0x0400` = BIT10 = `SYS_FUNC_CPU_ENABLE` (CPU 8051).
  Toggle off→on di BIT10 è sicuro (la USBA resta su) → 8051 riparte → firmware boota
  → `WINT_INIT_READY` (BIT6 MCUFWDL) si setta → `fw ready`.

Diagnosi via workflow multi-agente (rtl8xxxu regs.h: SYS_FUNC_USBA=BIT2,
SYS_FUNC_CPU_ENABLE=BIT10; ipotesi re-enumerazione refutata — Linux tiene lo stesso
usb_device per tutto download+reset_8051).

## Anche aggiunto (defense-in-depth)
**EP0 halt-recovery** in `control.rs`: `recover_ep0_halt` = Reset Endpoint (TRB 14)
+ re-init ring + Set TR Dequeue Pointer (TRB 16) su slot+DCI 1, invocato nei rami
d'errore di `control_in/out/out_data` quando completion code == 4 (transaction
error) o == 6 (stall). Prima ruos non aveva NESSUN recupero di endpoint halt; ora
ogni control transfer si auto-ripara da un halt invece di wedgare il pipe.

## Stato driver
SP1 (transport) ✓ · SP2 (power-on + fw download + 8051 boot) ✓ VALIDATI HW.
Prossimo: SP3c init MAC/BB/RF (radio TX/RX) → poi scan/assoc/data path.

## File toccati
- kernel/src/usb/wifi/mod.rs (bit fix + reset_8051 reale)
- kernel/src/usb/control.rs (EP0 halt-recovery + wiring)
