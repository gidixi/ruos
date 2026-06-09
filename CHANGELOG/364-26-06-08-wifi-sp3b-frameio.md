# 364 — WiFi RTL8188EU SP3b: bulk endpoint config + frame I/O

**Data:** 2026-06-08

## Cosa
`kernel/src/usb/wifi/mod.rs`:
- `WifiState` esteso (Copy, come `MscState`): tiene i ring bulk IN/OUT + cursori
  + pagina scratch.
- `configure_endpoints` — mirror di `msc::configure_endpoints`: alloca i ring,
  programma i context endpoint bulk IN/OUT, emette Configure Endpoint. Cablato
  nel dispatch di `device.rs` (dopo `configure_wifi`).
- `tx_desc_mgmt` (TxDesc 40B RTL) + `send_frame` (TxDesc + frame su bulk-OUT) +
  `recv_frame` (strip RxDesc 24B + drvinfo su bulk-IN).
- `scan` — broadcast probe-request poi poll dei beacon → `Vec<ScanResult>` (usa
  il layer 802.11 di SP3).
- `registry.rs` teardown: dealloc dei ring/scratch del WifiState.

## Perché
Plumbing di trasporto frame: necessario a scan e al data path. Mirrora il
pattern MSC già collaudato per la config endpoint xHCI.

## GAP funzionale (onesto)
`scan` non vedrà nulla finché manca l'**init BB/RF/MAC** (tabelle registro =
SP3c): senza, la radio non trasmette/riceve. Quelle tabelle sono centinaia di
register-poke magici version-specific — NON riprodotte qui blind (valori magici
errati = codice che sembra giusto ma è silenziosamente rotto, peggio che assenti).
Vanno prese dal reference `rtl8xxxu` e validate su HW.

## Stato / UNVERIFIED
Build `make iso` pulito. Tutto SP1-SP3b poggia su transport chip mai validato su
dongle reale. `send_frame`/`recv_frame`/`scan` sono API pronte ma non ancora
invocate (warning dead_code attesi).

## File toccati
- kernel/src/usb/wifi/mod.rs
- kernel/src/usb/device.rs
- kernel/src/usb/registry.rs
