# 383 — WiFi RTL8188EU SP2b: power-on sequence + bring-up wired

**Data:** 2026-06-08

## Cosa
- `kernel/src/usb/wifi/mod.rs`: `power_on` — sequenza `rtl8188eu_power_on` dal
  reference rtl8xxxu: clear HW power-down, poll power-ready (APS_FSMCO BIT17),
  release BB reset, gate AFE xtal, clear suspend/PCIe, enable MAC (poll
  self-clear), LPLDO bit4, infine `REG_CR = 0x063F` (abilita HCI/MAC DMA + code).
  Costanti registro prese da `regs.h` reale (REG_APS_FSMCO=0x04, REG_SYS_FUNC=0x02,
  REG_AFE_XTAL_CTRL=0x24, REG_LPLDO_CTRL=0x23, REG_CR=0x100).
- `bring_up` = `power_on` + `download_firmware`.
- Cablato in `configure_wifi`: dopo i log SP1, tenta `bring_up` (fallimento NON
  aborta il bind — lo slot resta registrato). Su HW questo produce il log
  completo del tentativo di bring-up.

## Perché
Completa SP2: il chip ora ha una sequenza power-on prima del firmware download
(senza, gli accessi registro fallivano). Cablato così la prossima
enumerazione su HW reale fornisce dati di validazione (quanto arriva la bring-up).

## Note / stato
- **UNVERIFIED su HW** (scelta utente: continuare blind). Tutta SP1+SP2 poggia su
  costanti/sequenze dal reference `rtl8xxxu`, mai eseguite su un dongle reale
  (passthrough QEMU impossibile da WSL2; VBox bloccato da Windows che tiene il
  device). Da validare appena il dongle è disponibile a ruos.
- Build `make iso` pulito.

## File toccati
- kernel/src/usb/wifi/mod.rs
