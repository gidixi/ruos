# 392 — WiFi RTL8188EU SP3c-4: RX enable

**Data:** 2026-06-09

## Cosa
`rx_enable` (cablato in `bring_up_radio` dopo init RF), fedele a rtl8xxxu
init_device RX block + rtl8188e_usb_quirks + rtl8188e_enable_rf:
- `REG_TRXFF_BNDY+2 = 0x25ff` (RX FIFO boundary — DEVE precedere il MAC-RX enable
  su 8188E, altrimenti latcha boundary errato).
- usb_quirks: `REG_CR |= CR_MAC_TX_ENABLE(BIT6)|CR_MAC_RX_ENABLE(BIT7)` +
  `REG_EARLY_MODE_CONTROL_8188E+3 = 0x01`.
- `REG_RX_DRVINFO_SZ(0x60f) = 4`.
- `REG_RCR(0x608) = 0x7000600E` (accept phys-match|mcast|bcast|mgmt + HTC_LOC +
  append phystat/icv/mic).
- `REG_MAR(0x620)/+4 = 0xffffffff` (accept-all multicast).
- `REG_FPGA0_RF_MODE(0x800) |= CCK(BIT24)|OFDM(BIT25)` (accende i modem baseband).
- enable_rf: `REG_RF_CTRL=0x07`, `REG_OFDM0_TRX_PATH_ENABLE(0xc04)` RX_A|TX_A,
  `REG_TXPAUSE(0x522)=0`.

Costanti/valori estratti verbatim da regs.h + 8188e.c locali. Per scan passivo si
salta il setup TX queue/LLT/boundary (non necessario alla sola RX, da ricerca).

## Stato
Build `make iso` pulito. Dopo questo la MAC può DMA-are i frame ricevuti al pipe
bulk-IN. NON ancora testabile end-to-end: `recv_frame` richiede i ring (creati in
configure_endpoints, post-enum) + la sintonia canale (config_channel, SP3c-6).
Verifica HW immediata: riga `wifi: rx enabled (...)` dopo `radio init done`,
boot prosegue (no wedge).

## Prossimo
SP3c-5 (fix parse RxDesc: rxdesc16, drvinfo bit16-19, de-aggregazione) + SP3c-6
(config_channel RF6052 0x18 + loop scan con i ring) → primi beacon decodificati.

## File toccati
- kernel/src/usb/wifi/mod.rs
