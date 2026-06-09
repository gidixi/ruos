# 390 — WiFi RTL8188EU SP3c-2: init register tables (MAC/PHY/AGC/RADIO_A)

**Data:** 2026-06-09

## Cosa
Portate **verbatim** le 4 tabelle di init del chip dal sorgente Linux mainline
`rtl8xxxu/8188e.c` in `kernel/src/usb/wifi/tables.rs`:
- `MAC_INIT` (rtl8188e_mac_init_table) — 93 entry `(u16, u8)`.
- `PHY_INIT` (rtl8188eu_phy_init_table, BB/PHY_REG) — 193 entry `(u16, u32)`.
- `AGC_TAB` (rtl8188e_agc_table) — 131 entry `(u16, u32)`.
- `RADIOA_INIT` (rtl8188eu_radioa_init_table) — 96 entry `(u8, u32)`, con marker
  delay `0xFE` mid-array (gestiti da `apply_rf_table`) + terminatore `(0xff,0xffffffff)`.

Estrazione **deterministica** (script `build/gen_tables.py`: download del raw
8188e.c + trasformazione regex C→Rust) — niente trascrizione a mano / parafrasi
LLM, così i valori sono byte-esatti. Terminatori inclusi nei dati, combaciano con
le condizioni di break delle `apply_*` (SP3c-1).

## Stato
Build `make iso` pulito. Dati pronti; verranno applicati da init_mac/init_phy_bb/
init_phy_rf in SP3c-3 (orchestrazione), che è anche dove il readback RF dovrebbe
diventare "vivo" (non più 0x00000).

## File toccati
- kernel/src/usb/wifi/tables.rs (nuovo, generato)
- kernel/src/usb/wifi/mod.rs (pub mod tables)
