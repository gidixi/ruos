# 427 — WiFi: supporto variante RTL8188FU (WIP)

**Data:** 2026-06-11

## Cosa
Primo supporto per il dongle RTL8188FU (PID 0xF179) accanto all'EU (0x8179):

- `tables_fu.rs` (nuovo): tabelle init MAC/PHY_REG/AGC/RADIO_A della variante FU.
- `fw/rtl8188fufw.bin` (nuovo): blob firmware FU; `download_firmware` sceglie
  EU/FU in base a `WifiState.is_fu` e itera il body con `chunks(FW_PAGE)`.
- `WifiState.is_fu` propagato in `bring_up`, `read_efuse_mac` (rtl_id atteso
  0x8181 per FU — da confermare su HW), `init_mac`/`init_phy_bb`/`init_phy_rf`
  e `bring_up_radio`, che selezionano le tabelle FU.

## Perché
Avere un secondo dongle supportato oltre all'RTL8188EU. WIP: rtl_id FU non
ancora verificato su hardware reale.

## File toccati
- kernel/src/usb/wifi/mod.rs
- kernel/src/usb/wifi/tables_fu.rs
- kernel/src/usb/wifi/fw/rtl8188fufw.bin
