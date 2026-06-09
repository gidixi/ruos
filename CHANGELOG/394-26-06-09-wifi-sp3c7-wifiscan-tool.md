# 394 — WiFi RTL8188EU SP3c-7: `wifiscan` tool + lazy bring-up

**Data:** 2026-06-09

## Cosa
Lo scan diventa un comando shell `wifiscan` (come `lsusb`/`disks`), e tutto il
chip bring-up si sposta dall'enumerazione al comando (lazy → boot veloce,
scan ri-lanciabile senza reboot).

- **Lazy:** `configure_wifi`/`configure_endpoints` ora fanno solo bind + endpoint
  (niente power-on/fw/radio al boot). `WifiState` ha un latch `radio_up`.
- **`wifi::run_scan(buf)`** (kernel entry): prende il controller (CTRLS) + copia
  fuori `UsbDevice` + `WifiState` dalla registry (pattern MSC), al primo giro fa
  `bring_up` (power-on+fw) + `bring_up_radio` (MAC/BB/RF) e setta `radio_up`, poi
  `scan`; riscrive i cursori EP0/ring; formatta `ssid\tch\tsec\n`.
- **registry**: accessor `first_wifi_slot` / `wifi_state` / `set_wifi_state` /
  `dev_copy` / `set_dev` (UsbDevice è Copy).
- **host fn** `ruos::wifi_scan(buf, cap) -> i32` (proc.rs, pattern `sata_list`) +
  registrazione nel linker wasmi.
- **tool** `user/wifiscan/` (front-end che stampa la tabella AP), aggiunto al
  workspace `user/Cargo.toml` + `BIN_TOOLS` del Makefile (→ `/bin/wifiscan.wasm`).
- **docs/api/ruos.md**: aggiunta voce `wifi_scan` (regola manutenzione).

## Verifica (HW reale)
Boot ora veloce (nessun bring-up wifi all'enum). In shell (locale o SSH):
```
wifiscan
SSID                              CH   SECURITY
<rete>                             6   wpa2
...
```
Primo `wifiscan` ~1-2s (bring-up); successivi rapidi (radio_up già true).

## File toccati
- kernel/src/usb/wifi/mod.rs (run_scan, lazy)
- kernel/src/usb/registry.rs (wifi/dev accessors)
- kernel/src/wasm/host/proc.rs (host fn)
- user/wifiscan/{Cargo.toml,src/main.rs} (nuovo tool)
- user/Cargo.toml, Makefile (build wiring)
- docs/api/ruos.md
