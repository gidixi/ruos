# 424 — WiFi RTL8188EU SP-WIFI-4 HW-validated: 4-way completo

**Data:** 2026-06-10

## Cosa
Fix che hanno fatto passare il 4-way handshake WPA2 su HW reale (prima
`4-way failed` / `no msg1`):

- **RCR += ACCEPT_DATA_FRAME (BIT11)** (`RCR_VAL` 0x7000600E→0x7000680E). La
  diagnostica HW mostrava `4way m1: timeout (rx total=75 data=0)`: con solo APM i
  data frame diretti a noi (l'EAPOL dell'AP) non arrivavano al bulk-IN. Con BIT11
  il chip accetta i data → msg1/msg3 arrivano. (L'"instabilità" attribuita prima
  a BIT11 era in realtà il panic DNS smoltcp + una chiavetta USB difettosa.)
- **`recv_eapol` a deadline wall-clock** (non più conteggio iterazioni): su canale
  affollato i beacon bruciavano il budget di 80 iter in <1s. Ora 3s reali per msg.
- **PMK pre-calcolato prima dell'auth** (PBKDF2 è lento ~1s) → ascolto msg1 subito
  dopo l'assoc, prima che i beacon riempiano la FIFO.
- **MIC msg3 verificato sui byte raw** (campo MIC azzerato), non su una
  ricostruzione (l'AP popola key_iv/key_rsc). + diagnostica per-step.

## Verifica (HW reale)
`wificonnect <ssid> <pw>` → `4way m1 key_info=0x008a` → `msg1 ok` → `msg2 sent
code=1` → `m3 key_info=0x13ca` → **`msg3 mic ok`** → **`4-way complete (gtk idx=2
len=16)`** → **`cam: ptk+gtk installed`**. WPA2-PSK autenticato + chiavi CCMP nel
CAM HW su driver from-scratch. SP-WIFI-3 (crypto) + SP-WIFI-4 (handshake+keys)
validati end-to-end.

## Prossimo
SP-WIFI-5: datapath cifrato (802.11↔802.3 + TXDESC_SEC_AES) + smoltcp WifiPhy +
DHCP + ping = link WiFi usabile. (Il link è autenticato ma non passa ancora IP.)

## File toccati
- kernel/src/usb/wifi/mod.rs
