# 397 — WiFi RTL8188EU SP-WIFI-1: TX-engine init + EFUSE MAC

**Data:** 2026-06-09

## Cosa
Il TxDesc 32B (entry 396) da solo non bastava: il `tx probe code=0` (timeout sul
bulk-OUT) e `mac=00:00:..` erano due bug separati, entrambi prerequisiti mancanti
del path TX, portati fedelmente da `rtl8xxxu_init_device`.

- **TX-engine** (senza, il packet-buffer on-chip non ha pagine → il MAC NAKa ogni
  bulk-OUT all'infinito → timeout). Split nello stesso ordine del reference:
  - *pre-firmware* (`tx_engine_pre_fw`, in `bring_up` dopo power-on): RQPN
    (`init_queue_reserved_page` → REG_RQPN_NPQ/REG_RQPN), priorità code
    (`init_queue_priority` → REG_TRXDMA_CTRL), RX-FIFO boundary (TRXFF_BNDY+2).
  - *post-RF* (`tx_engine_post_rf`, in `bring_up_radio` dopo init_phy_rf): TX
    buffer boundary (BCNQ/MGQ/WMAC_LBK/TRXFF/TDECTRL = total_page+1), PBP 128B,
    **LLT** (`init_llt_table`/`llt_write`), gen2 quirk (TXDMA_OFFSET_CHK |
    DROP_DATA_EN), TX-report ctrl/timer, FWHW_TXQ_CTRL (AMPDU_RETRY|XMIT_MGMT_ACK).
  - Layout code derivato da `nr_out_eps` (8188eu usa `config_endpoints_no_sie`);
    la coda MGNT mappa su out_ep[0] per qualunque conteggio → il nostro primo
    bulk-OUT è corretto. `WifiIface`/`WifiState` ora portano `nr_out_eps`,
    contato in `configure_wifi`.
- **EFUSE MAC** (`read_efuse8`/`read_efuse_mac`): il MAC non è in REG_MACID finché
  qualcuno non lo scrive (il chip non l'auto-carica). Decodifica la EFUSE nel map
  da 512B (formato header+word-mask), MAC del 8188eu a offset 0xd7 (gate rtl_id
  0x8129), poi scrive REG_MACID (così il filtro RX accetta le risposte dirette al
  nostro SA). Fallback al vecchio read di REG_MACID se la EFUSE non risponde.

## Perché
`tx code=0` + `mac=00:..`: TX morto perché il motore TX (pagine/LLT/boundary) non
era mai stato inizializzato (saltato per lo scan passivo), MAC nullo perché la
EFUSE non era mai letta. Con SA reale, anche l'active-scan trova SSID nascosti.

## Verifica (HW reale)
`wifiscan`: atteso `wifi: tx-engine pre-fw ...`, `wifi: tx-engine post-rf ...`,
`wifi: mac=[xx,..]` reale (non tutti 0), `wifi: tx probe code=1` (bulk-OUT ok).
`code=1` + mac reale = SP-WIFI-1 completo. `code=0` ancora → ricontrolla RQPN/LLT.

## Prossimo
SP-WIFI-2 (opmode STA + BSSID + auth open + assoc RSN + AID + H2C media-connect).

## File toccati
- kernel/src/usb/wifi/mod.rs
