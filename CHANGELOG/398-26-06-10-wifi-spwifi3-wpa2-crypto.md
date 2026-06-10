# 398 — WiFi RTL8188EU SP-WIFI-3: WPA2-PSK supplicant crypto

**Data:** 2026-06-10

## Cosa
Control-plane crypto del 4-way handshake WPA2-PSK (key-descriptor v2 =
HMAC-SHA1/AES, **non** SHA-256). Math pura offline, indipendente dal transport
RTL8188EU → verificabile senza dongle. CCMP resta HW-offload (key CAM), quindi
NO AES-CCM software: solo derivazione chiavi + autenticazione EAPOL + unwrap GTK.

`kernel/src/usb/wifi/wpa2.rs`:
- `pmk(pass, ssid)` = PBKDF2-HMAC-SHA1(4096, 32B).
- `derive_ptk(pmk, aa, spa, anonce, snonce)` = PRF-384 (sha1_prf:
  `label‖0x00‖data‖counter`, dato = min‖max MAC + min‖max nonce) → `Ptk {kck,kek,tk}`.
- `eapol_mic(kck, eapol)` = HMAC-SHA1[..16] (frame con MIC azzerato).
- `aes_unwrap(kek, wrapped)` = AES-128 key-unwrap RFC 3394 (sblocca la GTK del msg3).
- `selftest()` (boot-checks): vettori known-answer.

Dipendenze: `sha1` (force-soft), `hmac`, `aes` (soft via `aes_force_soft` già in
`.cargo/config.toml`).

## Perché
SP-WIFI-3 è l'unico pezzo a dipendenza-HW-zero → sviluppato e **verificato in
parallelo** mentre SP-WIFI-1 (TX) è in test su HW. Sbagliare PMK/PTK/MIC (SHA-1 vs
SHA-256, ordine min/max) = fallimento totale silenzioso → known-answer test obbligatorio.

## Verifica (offline, QEMU boot-checks)
`[T+1.870s] INFO usb wpa2 supplicant self-test ok (pmk + aes-unwrap vectors)`:
- PMK: ssid "IEEE" / pass "password" → `f42c6fc5..9710a12e` (IEEE 802.11i). Esercita
  tutta la catena PBKDF2/HMAC-SHA1 che PRF e MIC riusano.
- AES-unwrap: KEK 000102..0f, wrapped → `00112233..eeff` (RFC 3394 §4.1).

## Prossimo
SP-WIFI-2 (STA opmode + BSSID + auth + assoc RSN + AID + H2C media-connect) —
serve TX confermato (SP-WIFI-1). Poi SP-WIFI-4 (install chiavi CAM) usa `Ptk`/`aes_unwrap`.

## File toccati
- kernel/src/usb/wifi/wpa2.rs (nuovo)
- kernel/src/usb/wifi/mod.rs (mod wpa2)
- kernel/src/boot/phases/usb.rs (self-test boot-checks)
- kernel/Cargo.toml (sha1 + hmac + aes)
