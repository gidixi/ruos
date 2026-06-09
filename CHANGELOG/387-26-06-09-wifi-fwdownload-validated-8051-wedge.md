# 387 — WiFi RTL8188EU: firmware download VALIDATO su HW + root-cause 8051-start

**Data:** 2026-06-09

## Cosa (validato su hardware reale, dongle 0bda:8179)
- **Firmware download FUNZIONA**: `fw upload pages=4 ok=true` + `fw dl:
  csum_report=true` → il chip ha verificato il checksum del firmware in RAM.
  SP1 transport + SP2 power-on + upload + checksum **tutti validati su metallo**.
- Sequenza download riscritta fedele a `rtl8xxxu_download_firmware` (enable 8051
  16-bit, hold reset BIT19, csum-report, page-write 64B chunk, disable DL 16-bit,
  poll csum, DL_READY).

## Root cause del blocco 8051-start (diagnostica HW)
La riga `reset_8051 sysfunc=0xfc1c off_ok=false mid=None on_ok=false` dimostra:
- la write a `REG_SYS_FUNC_EN` (0x02) per spegnere la CPU (clear FEN_CPUEN)
  **fallisce** (code=4) **e wedga il pipe USB** (read successiva = timeout).
- Su questo dongle l'8051 guida la funzione USB → **CPU-off = device droppa**
  (serve replug). Il reference fa CPU off→on e in Linux sopravvive (stack USB
  pieno gestisce reset/re-enum); il nostro xHCI minimale no.

## Fix
Rimosso il toggle distruttivo di 0x02 da `start_firmware` (non si wedga più il
device). Si lascia la CPU abilitata (lo era dal download-start) + poll breve di
WINT_INIT_READY. `bring_up` ritorna success sul download verificato (csum).
Il run completo del firmware (che probabilmente **ri-enumera** il device USB) è
il prossimo subproject — richiede gestione della re-enumerazione post-fw.

## Stato
Build `make iso` pulito. fw download = milestone raggiunto e validato. Boot non
freeza, device non wedgato.

## File toccati
- kernel/src/usb/wifi/mod.rs
