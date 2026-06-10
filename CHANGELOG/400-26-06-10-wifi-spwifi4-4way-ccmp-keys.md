# 400 — WiFi RTL8188EU SP-WIFI-4: WPA2 4-way handshake + CCMP key install

**Data:** 2026-06-10

## Cosa
Il cuore della connessione: handshake 4-way EAPOL (usando la crypto SP-WIFI-3) +
install delle chiavi CCMP nel CAM HW. Dopo questo la rete è **autenticata**; manca
solo il datapath (SP-WIFI-5).

- `eapol.rs` (nuovo, framing 802.1X EAPOL-Key, network byte order): `parse`
  (key_info/replay/nonce/mic/key_data + **raw** bytes), `build` (MIC a 0,
  patchato dal chiamante), `extract_gtk` (KDE 00-0F-AC tipo 1 dai key data
  decifrati). key_info bits (ver2 = HMAC-SHA1/AES).
- `ieee80211.rs`: `build_eapol_data`/`parse_eapol_data` (802.11 data frame ToDS
  + LLC/SNAP 0x888E), `rsn_ie_wpa2_psk()` (stessa IE dell'assoc-req).
- `mod.rs`: `tx_desc` parametrizzato per QSEL (+ `send_data_frame` su QSEL_BE per
  EAPOL); `four_way_handshake` (recv msg1→SNonce(RNG)+derive_ptk→send msg2(MIC)→
  recv msg3(verifica MIC sui **byte raw** con campo MIC azzerato = check
  passphrase; unwrap GTK con KEK)→send msg4); `cam_write_key`
  (rtl8xxxu_cam_write: 6 dword/entry, REG_CAM_WRITE 0x674 + REG_CAM_CMD 0x670
  POLLING|WRITE); `enable_hw_security` (CR_SECURITY_ENABLE + REG_SECURITY_CFG
  0x680=0xCF). `connect` ora, se password presente: pmk→4-way→enable sec→install
  PTK(entry0,pairwise)+GTK(entry1,group). Output `4way=ok|failed|skipped`.

## Perché
CCMP è HW-offload → ruos fa solo control-plane: deriva chiavi + autentica EAPOL +
unwrap GTK, poi le chiavi vanno nel CAM e il chip cifra/decifra. MIC msg3 verificato
sui byte ESATTI ricevuti (non ricostruiti — l'AP può popolare key_iv/key_rsc) →
mismatch = passphrase errata.

## Verifica (HW reale)
`wificonnect <ssid> <password>` su WPA2 → netconsole: `auth ok`, `assoc ok aid=N`,
`media-connect sent`, `4-way complete (gtk idx=N len=16)`, `cam: ptk+gtk installed`.
Tool: `auth=ok assoc=ok aid=N 4way=ok`. `4-way: msg3 MIC mismatch` = password sbagliata.

## Prossimo
SP-WIFI-5: 802.11↔802.3 + smoltcp WifiPhy + TXDESC_SEC_AES sui data frame + DHCP +
ping = link WiFi usabile (coda + wifi_poll_task, mai CTRLS dentro NET.lock()).

## File toccati
- kernel/src/usb/wifi/eapol.rs (nuovo)
- kernel/src/usb/wifi/ieee80211.rs
- kernel/src/usb/wifi/mod.rs
- docs/api/ruos.md
