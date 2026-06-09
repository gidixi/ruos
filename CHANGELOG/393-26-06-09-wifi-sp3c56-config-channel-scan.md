# 393 — WiFi RTL8188EU SP3c-5/6: RxDesc fix + config_channel + passive scan

**Data:** 2026-06-09

## Cosa
Primo SCAN passivo: il driver sintonizza i canali 2.4 GHz e raccoglie i beacon.

- **Fix parse RxDesc** (`recv_frame`): rxdesc16 reale — `drvinfo_sz` in bit **16:19**
  (era letto `>>24`), aggiunto `shift` bit 24:25; frame parte a
  `RX_DESC_SIZE(24) + drvinfo*8 + shift`. + timeout configurabile.
- **`bulk_xfer_timeout`** (xhci/bulk.rs): variante con timeout scelto dal chiamante
  (lo scan polla bulk-IN con timeout breve 40ms → canali idle non bloccano 2s).
- **`config_channel`** (rtl8188eu_config_channel, 20 MHz): `REG_BW_OPMODE|=20MHz`,
  `FPGA0/1_RF_MODE &= ~bw-bit`, `RF6052 MODE_AG(0x18)` canale + BW 20MHz.
- **`scan`** (passivo): hop canali 1-13, `config_channel` + settle 15ms + ~12 poll
  bulk-IN, `parse_beacon`, colleziona AP unici, logga ogni SSID. Cablato a fine
  `configure_endpoints` (TEMP at enum; andrà dietro `wifiscan` lazy in SP3c-7).

Costanti/funzioni estratte verbatim dai sorgenti rtl8xxxu locali (8188e.c/regs.h/
rtl8xxxu.h: rxdesc16 layout, config_channel, MODE_AG masks, BW_OPMODE).

## Verifica (HW reale)
Boot col dongle: dopo `radio init done` + `rx enabled`, lo scan dovrebbe stampare
```
wifi: scan: ssid='<rete>' ch=N sec=wpa2
...
wifi: scan done: K AP(s)
```
= **primi SSID reali ricevuti e decodificati** → radio RX end-to-end funzionante.
Aggiunge ~2-6s all'enumerazione (TEMP).

## File toccati
- kernel/src/usb/wifi/mod.rs
- kernel/src/usb/xhci/bulk.rs
