# RTL8188EU USB-WiFi driver — resource gathering & decomposition

**Data:** 2026-06-08
**Branch:** `feat/usb-wifi-rtl8188eu`
**Stato:** raccolta risorse + decomposizione (PRE-design; il design/spec dei singoli
subproject arriva dopo, via brainstorming).

Device target: **Realtek RTL8188EUS** — `USB\VID_0BDA&PID_8179` (802.11n 1x1,
es. TP-Link TL-WN725N v2 / Edimax EW-7811Un). FullMAC parziale + softMAC host-side.

> ⚠️ Scala: il driver Linux `r8188eu` è ~50k righe. Un port "connetti a UN AP
> WPA2-PSK + data path" è multi-settimana, multi-subproject. Questo doc raccoglie
> i prerequisiti e spezza il lavoro; NON è il design.

## Risorse recuperate

### Firmware (✓ scaricato)
- File: `kernel/src/usb/wifi/fw/rtl8188eufw.bin`
- Size: 15262 byte. SHA256: `2ff74315287529dec2e50eb57d6e0c97d2116f28ae166773ccdf93b6360000c4`
- Header: signature `0x88e1` (little-endian dei primi 2 byte `e1 88`) → RTL8188E.
- Fonte: `https://gitlab.com/kernel-firmware/linux-firmware/-/raw/main/rtlwifi/rtl8188eufw.bin`
- Licenza: blob redistribuibile (linux-firmware, "Redistributable, no modification").
- Uso: caricato nella RAM del chip all'init via il path di firmware-download
  (8051 reset → write a FIFO → checksum/ready poll). Sarà `include_bytes!`.

### Trasporto USB (✓ già nel kernel)
- `kernel/src/usb/xhci/bulk.rs::bulk_xfer` (dal lavoro USB-MSC) → bulk IN/OUT.
- USB control transfer: `kernel/src/usb/control.rs` (per i vendor request del chip).
- USB core enumera già qualsiasi device e logga `dev 0bda:8179 class=.. numcfg=..`
  + interface/endpoint (device.rs). L'enumerazione NON richiede codice nuovo.

### Crypto WPA2 (da aggiungere — RustCrypto, no_std, già in uso sha2/pbkdf2)
- `aes` + `ccm` → **CCMP** (AES-CCM) encrypt/decrypt dei data frame.
- `hmac` + `sha1` → PRF per derivazione PTK dal PMK (+ `pbkdf2` già presente per
  PMK = PBKDF2-HMAC-SHA1(passphrase, ssid, 4096, 32)).
- Tutti no_std-compatibili e coerenti con lo stack crypto esistente (SSH).

### Riferimento driver (NON vendorizzato — consultare durante l'impl)
- Linux `drivers/staging/r8188eu/` (storico) e `drivers/net/wireless/realtek/rtl8xxxu/`
  per: mappa registri, tabelle init MAC/BB/RF, protocollo firmware-download,
  formati TX/RX descriptor (TxDesc/RxDesc del chip su USB bulk).
- Da consultare per i valori dei registri/tabelle init per-subproject; evitare
  copia massiva (GPL) — riprodurre solo le costanti hardware necessarie.

## Decomposizione in subproject (ognuno: spec → piano → impl)

1. **SP1 — Enumerazione + transport.** Match `0bda:8179` nella USB registry,
   bind un driver vendor-class, leggi config/endpoint, mappa bulk IN/OUT + i
   vendor control request per leggere/scrivere registri del chip. Deliverable:
   leggere un registro noto (es. chip version) e loggarlo. **De-risk gate.**
2. **SP2 — Firmware download + MAC/BB/RF init.** Power-on sequence, upload del
   blob, reset 8051, tabelle init (MAC/BB/RF), calibrazione. Deliverable: chip
   "up", RF acceso.
3. **SP3 — Scan.** Set channel, invia probe request / ricevi beacon, parse IE
   (SSID, RSN). Deliverable: lista SSID/canale/sicurezza loggata.
4. **SP4 — Associazione (open + WPA2-PSK).** Auth + assoc; per WPA2: EAPOL
   4-way handshake, derivazione PMK/PTK/GTK, install chiavi, CCMP. Deliverable:
   link L2 autenticato.
5. **SP5 — Data path → smoltcp.** TxDesc/RxDesc del chip, 802.11↔802.3,
   esponi un `smoltcp::phy::Device` come gli altri NIC; DHCP → IP. Deliverable:
   ping/DHCP via WiFi.

## Test di enumerazione (in corso, prima di SP1)

Passthrough del dongle fisico → QEMU per leggere descrittore + endpoint senza
codice:
- Windows (admin): `winget install usbipd`; `usbipd bind/attach --wsl --busid <id>`.
- WSL: `lsusb` mostra `0bda:8179`.
- QEMU: `make run` + `-device usb-host,vendorid=0x0bda,productid=0x8179` →
  seriale logga `usb: dev 0bda:8179 ...` + endpoint bulk.
Alternativa: dongle nella macchina ruos reale, log via netconsole/framebuffer.

## File toccati (finora)
- kernel/src/usb/wifi/fw/rtl8188eufw.bin (nuovo, firmware blob)
- docs/superpowers/specs/2026-06-08-rtl8188eu-wifi-resources.md (questo)
