# RTL8188EU WiFi — SP-WIFI-5: datapath cifrato + smoltcp + DHCP

**Data:** 2026-06-10
**Branch:** `feat/usb-wifi-rtl8188eu`
**Stato:** design (workflow understand+design), pre-implementazione

## Obiettivo
Dare un datapath IP al link WPA2 già associato + con chiavi CCMP nel CAM HW
(SP-WIFI-1..4 validati): smoltcp `phy::Device` sul dongle + DHCP + ping = WiFi
usabile. Milestone: `dhcp bound ip=...` su WiFi + `ping <gw>` con RTT.

## Vincolo centrale (lock)
`iface.poll()` di smoltcp gira sotto `NET.lock()` (net/mod.rs); i bulk transfer
USB richiedono `&mut Xhci` dietro `CTRLS` (IrqMutex, IF-masking, busy-poll
`event::wait_for`). **CTRLS e NET non devono MAI essere tenuti insieme** (oggi
nessuno lo fa). Soluzione: **disaccoppiare con due code in RAM**, mai annidare.

## Architettura
- **WIFI_RX / WIFI_TX**: `IrqMutex<VecDeque<Vec<u8>>>` (frame Ethernet), bounded
  (DEPTH 64). RX drop-oldest, TX drop-new su overflow. **Leaf lock**: push/pop +
  drop guard subito; mai chiamare USB/smoltcp/NET tenendole. `ATTACHED: AtomicBool`.
- **(A) `wifi_poll_task`** (BSP executor): fa TUTTA l'I/O USB. Replica run_scan
  (first_wifi_slot → CTRLS.lock → wifi_state/dev_copy out, SLOTS rilasciato) →
  loop bounded: drena WIFI_TX (encap 802.3→802.11+CCMP → send_data_frame) + riempie
  WIFI_RX (recv_data 40ms → decap → rx_push) → set_wifi_state/set_dev → drop CTRLS
  → await. Tocca CTRLS + le code leaf, **mai NET**.
- **(B) `WifiPhy`** (smoltcp Device, Ethernet/MTU1500): `receive` pop WIFI_RX,
  `transmit`/consume push WIFI_TX. **Zero USB**, solo le code leaf → `net::poll`
  (NET tenuto) non raggiunge mai CTRLS.
- Le due si toccano SOLO via le code (sempre innermost) → nessun ciclo.
- 802.11/CCMP/BSSID vivono SOLO sul lato USB (wifi_poll_task); WifiPhy è un
  mover di frame Ethernet "stupido".
- iface/Device creati **lazy** dopo connect riuscito: `net::attach_wifi(mac)` da
  `run_connect` quando `four_way==Some(true)`.

## TX path
smoltcp TxToken → frame Ethernet → push WIFI_TX → (wifi_poll_task) tx_pop →
costruisci CCMP header da `st.tx_pn` (48-bit monotono, ExtIV, key_id 0) →
`build_data_frame` (FC Data ToDS, **Addr1=RA=BSSID**, Addr2=TA=mac, Addr3=DA=eth-dst;
CCMP hdr; LLC/SNAP+ethertype; payload) → `send_data_frame(encrypt=true)`:
`tx_desc` OR `TXDESC_SEC_AES`(0x00c00000) in txdw1, **csum ricalcolato per ultimo**
→ bulk-OUT. Il chip cifra con la PTK del CAM (selezione per MAC) + appende MIC.
**Tutto il TX STA→AP usa Addr1=BSSID + chiave pairwise** (anche per dst broadcast:
DHCP/ARP vanno con Addr3=broadcast ma Addr1=BSSID — comportamento STA normale).

## RX path
(wifi_poll_task) recv_data → `parse_rxdesc` (pkt_len/drvinfo/shift + crc32 bit14,
icverr bit15, swdec, security 3-bit, rpt_sel) → **accetta solo crc32==0 &&
icverr==0 && swdec==0 && security==AES(4)**, skip rpt_sel (C2H) → copy-out cleartext
802.11 (include CCMP hdr 8B + MIC 8B, esclude FCS) → `parse_data_frame` (strip MAC
hdr 24/26 + CCMP 8 + LLC/SNAP 8, trim MIC 8 in coda → Ethernet-II dst=Addr1 src=Addr3
ethertype) → rx_push → (net::poll) WifiPhy::receive → smoltcp.

## DHCP / ping
`attach_wifi(mac)`: sotto NET, build `iface_wifi` Ethernet + dhcpv4 socket dedicato
in net_sockets + `set_attached(true)`. `net::poll` aggiunge il 3° ramo
`iface_wifi.poll`. DHCP DISCOVER → WIFI_TX → radio; OFFER/ACK → WIFI_RX → socket →
lease applicato a iface_wifi (ip/route/dns cap-1). `ping` (icmp.rs): estendere la
guard per accettare `iface_wifi`. **Solo se nessun NIC wired è bound** (evita
doppia default route).

## Rischi
- PN TX monotono per-chiave (replay); persiste in WifiState; key_id+ExtIV corretti.
- Broadcast TX = pairwise key Addr1=BSSID (STA normale) — niente group-TX.
- MTU+overhead vs scratch 4096 (ok per 1500); st.data è UNA pagina → TX e RX
  serializzati (drena TX poi riempi RX).
- Coesistenza NIC wired vs WiFi: attach solo se nessun wired bound.
- csum dopo SEC = ultimo (altrimenti TX si blocca).
- GTK-rekey EAPOL in sessione droppato (out of scope).

## Task (commit-sized)
T1 WifiState fields (bssid/associated/tx_pn) · T2 TX SEC desc · T3 ccmp_header+PN ·
T4 build/parse_data_frame · T5 RX classify (parse_rxdesc+recv_data) · T6 datapath.rs
code · T7 WifiPhy Device · T8 poll_io · T9 spawn wifi_poll_task · T10 NetState+attach_wifi ·
T11 net::poll 3° ramo + DHCP apply · T12 run_connect→attach_wifi · T13 ping guard ·
T14 HW test (dhcp+ping su WiFi) + CHANGELOG.
