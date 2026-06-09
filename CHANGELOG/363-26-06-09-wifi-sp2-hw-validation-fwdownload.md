# 363 — WiFi RTL8188EU: validazione HW reale + firmware download corretto

**Data:** 2026-06-09

## Cosa
Primo test su **hardware reale** (boot ruos col dongle 0bda:8179, log via
seriale/framebuffer) → validazione + correzione del firmware download.

**Validato su HW:**
- SP1 transport reale: `wifi: rtl8188eu sys_cfg=0x24403735 (transport ok)` —
  i vendor request register-read FUNZIONANO (non era blind-rotto).
- Endpoint discovery: `iface=0 ep_in=0x81 ep_out=0x02 mps=512`.
- power-on: `power_on done (pwr_rdy=true)` (poll APS_FSMCO ok).
- firmware blob: `fw 15262 bytes, body 15230 (sig 0x88e1)`; upload FIFO + MCUFWDL
  (readback 0x05 = EN|CSUM) OK.

**Bug trovato + fix (evidence-driven):**
- La scrittura finale a `REG_SYS_FUNC_EN (0x02)` per il restart 8051 dava
  `control_out_data code=4` (USB transaction error). Causa: sequenza
  post-download sbagliata (toggle SYS_FUNC_EN come primario).
- Riscritto `download_firmware` **fedele al reference rtl8xxxu**
  (`rtl8xxxu_download_firmware` + `start_firmware`), preso verbatim dal kernel:
  - enter: enable 8051 (16-bit SYS_FUNC) → clear RAM-sel → enable MCU DL →
    reset 8051 (clear BIT19 del MCUFWDL 32-bit) → arm checksum.
  - write pagine a chunk da 64B (un control-OUT da 4 KiB dà transaction error,
    EP0 mps=64 — verificato su HW).
  - exit: disable MCU DL (16-bit).
  - start: poll CSUM_REPORT → set DL_READY/clear WINT_INIT_READY (32-bit) →
    reset_8051 (RSV_CTRL=0 + toggle FEN_CPUEN 16-bit) → poll WINT_INIT_READY.
- Aggiunta diagnostica: `reg_write` logga addr/len sul fallimento.

## Stato
Build `make iso` pulito. Fix NON ancora ri-testato su HW (utente offline) —
atteso `wifi: fw ready` al prossimo boot = SP2 completo+validato.

## File toccati
- kernel/src/usb/wifi/mod.rs
