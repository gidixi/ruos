# 389 — WiFi RTL8188EU SP3c-1: RF register access (LSSI) + delays + table scaffolding

**Data:** 2026-06-09

## Cosa
Primo sub-progetto del radio bring-up (SP3c). I registri RF NON sono nello spazio
MAC (vendor EP0): vivono dietro un bridge seriale BB (LSSI/HSSI 3-wire). Aggiunto
l'accesso + le primitive comuni a tutto SP3c.

- `kernel/src/boot/clock.rs`: `udelay`/`mdelay` busy-wait sul TSC (bounded; la
  `Delay` async è inusabile nel bring-up sincrono, e le tabelle RADIO_A hanno
  marker di delay). Riusa l'esistente `tsc_per_ms()`.
- `kernel/src/usb/wifi/mod.rs`:
  - costanti FPGA0 LSSI/HSSI path A (offset+shift+mask da rtl8xxxu regs.h).
  - `write_rfreg(reg,val)` / `read_rfreg(reg)` — verbatim da rtl8xxxu
    write_rfreg/read_rfreg (encoding 20-bit, sequenza EDGE_READ + readback PI/SI).
  - scaffolding `apply_reg8_table` / `apply_reg32_table` / `apply_rf_table`
    (terminatori + marker delay), pronte per i dati delle tabelle (SP3c-2).
  - `rf_selftest` — legge rf[0x00]/rf[0x18] + logga, cablato in `configure_wifi`
    dopo `fw ready`: prova che il path RF/FPGA0 è raggiungibile e NON wedga EP0.

## Decomposizione SP3c
Vedi `docs/superpowers/specs/2026-06-09-rtl8188eu-wifi-sp3c-radio-bringup-design.md`.
Percorso a scan passivo: SP3c-1 (questo) → SP3c-2 tabelle → SP3c-3 init mac/bb/rf →
SP3c-4 RX enable → SP3c-5 fix RxDesc → SP3c-6 config_channel + channel-hop scan.
Bring-up pesante (tabelle+msleep) sarà LAZY su comando `wifiscan` (SP3c-3+); per
SP3c-1 il self-test è cheap e gira una volta all'enumerazione.

## Verifica (HW reale)
Boot col dongle → dopo `fw ready`: `wifi: rf selftest rf[0x00]=0x..... rf[0x18]=
0x..... tsc_per_ms=N`, e il boot prosegue (EP0 non wedgato). Pre-BB-init i valori
RF possono essere 0/garbage — il punto è che il path gira e il device resta vivo.

## File toccati
- kernel/src/boot/clock.rs
- kernel/src/usb/wifi/mod.rs
- docs/superpowers/specs/2026-06-09-rtl8188eu-wifi-sp3c-radio-bringup-design.md (nuovo)
