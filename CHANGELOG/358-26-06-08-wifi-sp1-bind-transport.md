# 358 — WiFi RTL8188EU SP1: bind + register transport

**Data:** 2026-06-08

## Cosa
Primo subproject del driver USB-WiFi Realtek RTL8188EU (`0bda:8179`), su branch
`feat/usb-wifi-rtl8188eu`. Nuovo modulo `kernel/src/usb/wifi/mod.rs`:
- `configure_wifi`: match VID/PID `0x0BDA/0x8179` all'enumerazione, scoperta
  endpoint bulk IN/OUT, SET_CONFIGURATION.
- Register I/O via control-IN vendor (protocollo rtl8xxxu: `bmRequestType=0xC0`,
  `bRequest=0x05`, `wValue=offset`): `reg_read8/16/32`.
- Deliverable de-risk: legge `REG_SYS_CFG` (0x00F0) e logga la chip-version
  → prova che il transport raggiunge il chip.
- Variante `SlotKind::Wifi(WifiState)` nella registry (endpoint salvati per SP2);
  dispatch in `device.rs` dopo il ramo MSC.

Firmware blob `kernel/src/usb/wifi/fw/rtl8188eufw.bin` (15262 B) già recuperato
per SP2 (download nel chip).

## Perché
Fondamenta del driver WiFi. SP1 isolato e verificabile: se su HW reale compare
`wifi: rtl8188eu sys_cfg=0x...`, il control-pipe + vendor protocol funzionano e si
può procedere con SP2 (firmware download + init RF). NIENTE firmware/scan/assoc qui.

## Note
Verifica SP1 = build-only ora (test enumerazione passthrough saltato per scelta).
Le costanti vendor request + offset registri vengono dal driver di riferimento
`rtl8xxxu`/`r8188eu`, da validare su hardware reale.

## File toccati
- kernel/src/usb/wifi/mod.rs (nuovo)
- kernel/src/usb/mod.rs
- kernel/src/usb/registry.rs
- kernel/src/usb/device.rs
- docs/superpowers/specs/2026-06-08-rtl8188eu-wifi-sp1-design.md
- docs/superpowers/specs/2026-06-08-rtl8188eu-wifi-resources.md
