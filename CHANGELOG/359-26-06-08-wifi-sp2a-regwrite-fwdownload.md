# 359 — WiFi RTL8188EU SP2a: reg-write transport + firmware download

**Data:** 2026-06-08

## Cosa
Fondamenta di scrittura per il driver WiFi (branch `feat/usb-wifi-rtl8188eu`):
- `kernel/src/usb/control.rs`: nuova `control_out_data` — control-OUT con data
  stage (Setup TRT=2 OUT + Data OUT + Status IN). Serviva per le scritture
  registro vendor (l'esistente `control_out` è no-data).
- `kernel/src/usb/wifi/mod.rs`: `reg_write8/16/32` + `reg_set8` (RMW) via
  `control_out_data` (vendor `bmRequestType=0x40`, `bRequest=0x05`).
- Firmware embeddato: `include_bytes!("fw/rtl8188eufw.bin")` (15262 B).
- `download_firmware`: sequenza RTL8188E da reference rtl8xxxu — abilita 8051 +
  MCUFWDL, scrive il blob (header 32B saltato) pagina-per-pagina (4 KiB) nel FIFO
  `REG_FW_START_ADDR` con page-select in `MCUFWDL+2`, esce dal download mode,
  riavvia l'8051, poll `WINTINI_RDY`.

## Perché
`reg_write` è richiesto da TUTTO l'init del chip (firmware, RF/BB/MAC). SP2a porta
la primitiva (corretta + build-verificata) e struttura il firmware download.

## Note / stato
- `download_firmware` NON è ancora cablato nel path di enumerazione (richiede
  prima la power-on sequence — tabella registri = SP2b — altrimenti gli accessi
  registro vanno in timeout). Esposto `pub` per SP2b.
- **UNVERIFIED su HW**: il transport SP1 (vendor reg I/O) non è ancora stato
  confermato su un dongle reale (passthrough VBox bloccato: Windows tiene il
  device). Costanti/sequenza dal reference `rtl8xxxu`/`r8188eu`, da validare.
- Build `make iso` pulito (warning residui preesistenti, non del wifi).

## File toccati
- kernel/src/usb/control.rs
- kernel/src/usb/wifi/mod.rs
