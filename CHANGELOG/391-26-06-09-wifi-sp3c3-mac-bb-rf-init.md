# 391 — WiFi RTL8188EU SP3c-3: MAC/BB/RF init orchestration

**Data:** 2026-06-09

## Cosa
Orchestrazione dell'init radio dopo `fw ready`, fedele a rtl8xxxu:
- `init_mac` (rtl8xxxu_init_mac): applica `MAC_INIT` + `REG_MAX_AGGR_NUM(0x4CA)=0x0707` (8188E).
- `init_phy_bb` (rtl8188eu_init_phy_bb): release reset BB/RF
  (`REG_SYS_FUNC |= BB_GLB_RSTN|BBRSTB|DIO_RF`; `REG_RF_CTRL(0x1F)=0x07`;
  `REG_SYS_FUNC = USBA|USBD|BB_GLB_RSTN|BBRSTB`), poi tabelle `PHY_INIT` + `AGC_TAB`.
- `init_phy_rf` (rtl8xxxu_init_phy_rf, RF_A): preambolo RF_INT_OE(0x860) BIT20/BIT4
  + HSSI_PARM2 clear 3WIRE_ADDR/DATA_LEN, applica `RADIOA_INIT` (con marker delay
  0xFE→msleep — `apply_rf_table` identica a init_rf_regs), poi restore RFENV su
  RF_SW_CTRL(0x870).
- `bring_up_radio` = init_mac → init_phy_bb → init_phy_rf, cablato in
  `configure_wifi` prima del self-test RF.

Costanti bit estratte verbatim da regs.h (SYS_FUNC_BB_GLB_RSTN=BIT1, BBRSTB=BIT0,
DIO_RF=BIT13, USBA=BIT2, USBD=BIT4, RF_ENABLE|RSTB|SDMRSTB=0x07, FPGA0_RF_RFENV=
BIT4, 3WIRE_ADDR/DATA_LEN=0x400/0x800). init_phy_regs/init_rf_regs verificate
identiche alle nostre apply_*.

## Verifica (HW reale)
Dopo `fw ready` + `radio init: mac/bb/rf` + `radio init done`, il
`rf selftest rf[0x00]=.. rf[0x18]=..` dovrebbe ora dare valori **vivi** (≠0x00000),
provando che MAC/BB/RF init ha acceso il synthesizer RF. NOTA: ~0.5-1s aggiunti
all'enumerazione (≈420 scritture + msleep RADIO_A) — temporaneo, andrà dietro un
comando `wifiscan` lazy (SP3c-6/7).

## File toccati
- kernel/src/usb/wifi/mod.rs
