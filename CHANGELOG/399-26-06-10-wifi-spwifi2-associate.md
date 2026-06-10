# 399 — WiFi RTL8188EU SP-WIFI-2: STA auth + WPA2 association

**Data:** 2026-06-10

## Cosa
MLME di connessione: open-system auth + association WPA2 (fino all'AID + join-bss).
Il TX confermato in SP-WIFI-1 (`tx probe code=1`) ha sbloccato questo step.

- `ieee80211.rs` (frame standard, indipendenti dal chip — in Linux stanno in
  mac80211): `build_auth_request` (open, algo 0 seq 1), `build_assoc_request`
  (cap ESS|Privacy + listen-int + SSID + supp-rates + **RSN IE WPA2-PSK/CCMP**
  00-0F-AC: group/pairwise CCMP, AKM PSK — senza, un AP WPA2 rifiuta),
  `parse_auth_response`/`parse_assoc_response` (status + AID).
- `mod.rs` (registri chip, fedele a rtl8xxxu): `set_opmode_sta` (REG_MSR port0
  link STA), `set_bssid_reg` (REG_BSSID 0x618), `h2c_cmd` (mailbox gen2:
  REG_HMBOX_0 0x1d0, poll REG_HMTFR 0x1cc), `report_connect` (H2C
  MEDIA_STATUS_RPT cmd 0x01, parm connect|role_AP<<4). `connect()`: scan→trova
  AP (BSSID/canale)→config_channel→opmode STA+BSSID→auth (send/recv retry)→assoc
  →AID (REG_BCN_PSR_RPT 0x6a8 = 0xc000|aid) + REG_BCN_MAX_ERR=0xff + H2C
  media-connect. `run_connect` (pattern run_scan: grab CTRLS, copy dev/state,
  lazy bring-up via `ensure_radio_up` factor-out, write-back).
- Host fn `ruos::wifi_connect(ssid,pass,buf)` + tool `wificonnect <ssid> [pass]`
  (workspace + Makefile + limine.conf minimal bundle) + docs/api/ruos.md.

## Perché
SP-WIFI-2 = il pezzo che mancava per provare auth/assoc, ora verificabile (TX
vivo). Password accettata ma INUTILIZZATA: il 4-way handshake (SP-WIFI-3, crypto
già pronta) + install chiavi CCMP (SP-WIFI-4) non sono ancora collegati → la rete
si **associa** ma non passa traffico. `assoc=ok aid=N` prova la catena MLME.

## Verifica (HW reale)
`wificonnect <ssid> <password>` su WPA2 → netconsole: `connect: ap [..] ch=N`,
`auth ok`, `assoc ok aid=N`, `media-connect sent`. Tool stampa `auth=ok assoc=ok
aid=N`. NB: l'AP può deautenticare dopo qualche secondo senza 4-way — l'assoc-resp
con AID arriva PRIMA, è quello il milestone.

## Prossimo
SP-WIFI-4 collega `wpa2::derive_ptk`/`aes_unwrap` (SP-WIFI-3) al 4-way EAPOL +
install chiavi nel CAM → poi SP-WIFI-5 datapath + DHCP.

## File toccati
- kernel/src/usb/wifi/ieee80211.rs
- kernel/src/usb/wifi/mod.rs
- kernel/src/wasm/host/proc.rs
- user/wificonnect/{Cargo.toml,src/main.rs} (nuovo)
- user/Cargo.toml, Makefile, limine.conf
- docs/api/ruos.md
