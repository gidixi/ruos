# 396 — WiFi RTL8188EU SP-WIFI-1: txdesc32 fix + active probe

**Data:** 2026-06-09

## Cosa
Primo passo verso la connessione: il TX era **silenziosamente morto** (TxDesc
sbagliato). Fix fedele a `rtl8xxxu_txdesc32` (8188eu).

- `TX_DESC_SIZE 40→32`. `tx_desc_mgmt(frame_len, seq, bcast)` riscritto col byte
  map corretto: pkt_size, pkt_offset=32, **txdw0 byte[3]=OWN(0x80)|FSG|LSG**
  (+BMC se bcast), txdw1 QSEL<<8, txdw2 antenna, txdw3 seq<<16, txdw4
  USE_DRIVER_RATE, txdw5 rate1M+retry, **csum XOR (calcolato per ultimo)**.
  Prima: OWN mai settato + flag come bit di word 32-bit + niente csum → il chip
  non prendeva mai il descrittore.
- `tx_desc_csum` (XOR 16 word). `WifiState { tx_seq, mac }`; seq 12-bit scritto
  sia in txdw3 sia nell'header 802.11. MAC letto da `REG_MACID`(0x610) al bring-up.
- `send_frame(x, st, frame, seq, bcast) -> u8` (codice completamento).
- `build_probe_request(sa, da, ssid, seq)` parametrizzato (DA dirigibile + seq).
- `scan` ora fa **active scan**: broadcast probe-request per canale + logga il
  primo `tx code`.

## Verifica (HW reale)
`wifiscan`: `wifi: mac=[..]` + `wifi: tx probe code=1` (1=bulk-OUT ok → TX raggiunge
l'antenna) + lista reti (active scan trova anche SSID nascosti). `code=1` = TxDesc
corretto. `code!=1` o zero risposte → ricontrolla OWN/csum/offset.

## Prossimo
SP-WIFI-2 (opmode/BSSID + auth + assoc + AID + H2C media-connect),
SP-WIFI-3 (4-way WPA2), SP-WIFI-4 (key CAM/CCMP HW), SP-WIFI-5 (smoltcp+DHCP).
Decomposizione: `docs/superpowers/specs/2026-06-09-rtl8188eu-wifi-sp4-connect-design.md`.

## File toccati
- kernel/src/usb/wifi/mod.rs
- kernel/src/usb/wifi/ieee80211.rs
- docs/superpowers/specs/2026-06-09-rtl8188eu-wifi-sp4-connect-design.md (nuovo)
