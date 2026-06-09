# RTL8188EU WiFi — connect to WPA2 (SP4/SP5) design + SP-WIFI-1

**Data:** 2026-06-09
**Branch:** `feat/usb-wifi-rtl8188eu`
**Stato:** approvato (decomposizione + SP-WIFI-1), pre-implementazione

## Contesto
Lo scan passivo funziona su HW (`wifiscan` trova le reti). Obiettivo: connettersi a
una rete **WPA2-PSK** e ottenere DHCP + ping. Flusso reale mappato da rtl8xxxu via
workflow di ricerca.

**Chiave:** **CCMP è HW-offload** (security CAM) → ruos NON implementa AES-CCM
software; installa solo le chiavi e il chip cifra/decifra. ruos non ha mac80211 né
wpa_supplicant → implementa MLME (auth/assoc) + supplicant 4-way in casa.

**Crypto da aggiungere** (solo control-plane 4-way, no per-pacchetto): `sha1` +
`hmac` + `aes` (RustCrypto, force-soft come sha2). **WPA2-PSK usa SHA-1 non SHA-256**
(PMK=PBKDF2-HMAC-SHA1, PTK-PRF + MIC = HMAC-SHA1; GTK unwrap = AES-128 RFC3394).

## Decomposizione
| SP | Cosa | Milestone (serial/netconsole) |
|----|------|-------------------------------|
| **SP-WIFI-1** | fix TxDesc 32B + probe attivo round-trip | `tx code=1` + probe-resp dall'AP |
| SP-WIFI-2 | opmode STA + BSSID + auth open + assoc(RSN IE) + AID + H2C media-connect | `auth ok`→`assoc ok aid=N`→`media-status sent` |
| SP-WIFI-3 | supplicant WPA2 4-way (host; unit-test offline su vettori noti) | vettori OK; poi `4-way complete` |
| SP-WIFI-4 | install chiavi security CAM + datapath cifrato (HW CCMP) | `cam: ptk+gtk installed`, ARP reply cifrata |
| SP-WIFI-5 | 802.11↔802.3 + smoltcp WifiPhy + DHCP + ping | **`dhcp lease` + ping** 🎯 |

## SP-WIFI-1 — fix TxDesc (il TX oggi è morto)
Il `tx_desc_mgmt` attuale (40B) è rotto: flag impacchettati come bit di un word
32-bit, **OWN mai settato**, **niente checksum** → il chip non prende il descrittore.
Tutto (auth/assoc/EAPOL) passa di qui.

**`rtl8xxxu_txdesc32` (32 byte, LE):**
- `[0..2]` pkt_size = frame_len (payload, no desc)
- `[2]` pkt_offset = 32
- `[3]` txdw0 (u8) = OWN(0x80)|FIRST_SEG(0x08)|LAST_SEG(0x04) (+BMC 0x01 se DA bcast/mcast)
- `[4..8]` txdw1 = QSEL_MGMT(0x12)<<8 = 0x00001200
- `[8..12]` txdw2 = ANT_A(BIT24)|ANT_B(BIT25)|AGG_BREAK(BIT16) = 0x03010000
- `[12..16]` txdw3 = (seq & 0xFFF)<<16
- `[16..20]` txdw4 = USE_DRIVER_RATE(BIT8) = 0x00000100
- `[20..24]` txdw5 = rate(0=1M) | RETRY_LIMIT_ENABLE(BIT17) | (6<<18) = 0x001A0000
- `[24..28]` txdw6 = 0
- `[28..30]` csum = XOR dei 16 word u16 (con csum=0), calcolato **per ultimo**
- `[30..32]` txdw7 = ANT_C(BIT29)>>16 = 0x2000

**Seq:** `WifiState.tx_seq` (12-bit), scritto SIA nell'header 802.11 (seq_ctrl =
sn<<4) SIA in txdw3 (sn<<16) — devono coincidere.

**MAC:** leggere `REG_MACID` (0x0610, 6 byte) all'init → `WifiState.mac` (SA del probe).

**Modifiche:**
- `mod.rs`: `TX_DESC_SIZE 40→32`; riscrivi `tx_desc_mgmt(frame_len, seq, bcast)`
  con la byte map + `tx_desc_csum`; `WifiState { tx_seq: u16, mac: [u8;6] }`; leggi
  REG_MACID in `bring_up_radio`; `send_frame(x, st, frame, seq, bcast) -> u8` (code);
  `active_probe` helper; `scan` invia un probe-request attivo per canale + logga il
  primo tx code.
- `ieee80211.rs`: `build_probe_request(sa, da, ssid, seq)` (DA dirigibile + seq_ctrl).

**Verifica HW:** boot + `wifiscan` → `wifi: tx code=1` (bulk-OUT ok) + risposte
probe ricevute (active scan trova reti anche con SSID nascosto). Round-trip = TX
raggiunge l'antenna. Se `tx code=1` ma zero risposte → ricontrolla OWN/csum/offset.

**File:** `kernel/src/usb/wifi/mod.rs`, `kernel/src/usb/wifi/ieee80211.rs`.

## Rischi (dal workflow)
- TxDesc è il gap più alto, blocca tutto: OWN al byte 3, csum per ultimo, pkt_offset=32.
- WPA2-PSK = SHA-1 (non SHA-256): PMK/PTK/MIC sbagliati = fallimento totale silenzioso (SP-WIFI-3).
- H2C 8188eu = gen2 `MEDIA_STATUS_RPT` (0x01), non gen1 JOIN_BSS (SP-WIFI-2).
- Ordine: REG_MSR+REG_BSSID PRIMA di auth; AID/security DOPO assoc/msg3 (SP-WIFI-2/4).
- send/recv sincroni dietro CTRLS lock → SP-WIFI-5 usa una coda + wifi_poll_task, mai CTRLS attraverso NET.lock().
