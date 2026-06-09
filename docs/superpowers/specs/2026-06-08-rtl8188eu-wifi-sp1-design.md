# RTL8188EU WiFi — SP1 (bind + transport) design

**Data:** 2026-06-08
**Branch:** `feat/usb-wifi-rtl8188eu`
**Stato:** approvato, pre-implementazione
**Parent:** `2026-06-08-rtl8188eu-wifi-resources.md` (decomposizione SP1-5)

## Scope SP1

Primo subproject del driver WiFi, **de-risk gate**: riconoscere il dongle
`0bda:8179` durante l'enumerazione USB, stabilire l'I/O sui registri del chip via
USB control transfer (protocollo vendor rtl8xxxu) e leggere `REG_SYS_CFG` per
provare che il transport funziona.

**In scope:**
- Bind del device per **VID 0x0BDA && PID 0x8179** (non genericamente class 0xFF).
- Helper `reg_read8/16/32` via control-IN vendor request.
- Scoperta + salvataggio degli endpoint **bulk IN/OUT** (serviranno in SP2).
- Lettura di `REG_SYS_CFG` (0x00F0) + log della chip-version.
- Variante `SlotKind::Wifi` nella registry.

**Out of scope (SP2+):** `reg_write` (richiede control-OUT con data stage),
firmware download, init RF/BB/MAC, scan, associazione, data path. Nessun uso degli
endpoint bulk in SP1 (solo scoperti e salvati).

## Vincoli / rischi

- **Nessun test di enumerazione live** (passthrough saltato per scelta utente):
  verifica SP1 = **build-only** ora; la stringa di log si valida su HW reale dopo.
- I valori vengono dal driver di riferimento `rtl8xxxu`/`r8188eu`, marcati
  "verify on HW":
  - vendor request register-read: `bmRequestType=0xC0` (Dev→Host,Vendor,Device),
    `bRequest=0x05`, `wValue=reg_offset`, `wIndex=0`, data 1/2/4 byte LE.
  - `REG_SYS_CFG = 0x00F0`.

## Architettura

### Nuovo file `kernel/src/usb/wifi/mod.rs`

```rust
/// Endpoint/iface layout scoperto all'enumerazione (bulk usati da SP2).
pub struct WifiIface {
    pub iface:   u8,
    pub ep_in:   u8,
    pub ep_out:  u8,
    pub mps_in:  u16,
    pub mps_out: u16,
}

/// Stato per-slot salvato nella registry.
#[derive(Clone, Copy)]
pub struct WifiState {
    pub iface:   u8,
    pub ep_in:   u8,
    pub ep_out:  u8,
    pub mps_in:  u16,
    pub mps_out: u16,
}
```

**Register I/O** (control-IN vendor, su `dev.ep0`):
- `reg_read32(x, dev, addr) -> Option<u32>`: `Setup { req_type:0xC0, request:0x05,
  value:addr, index:0, length:4 }` → `control_in` in un `DmaRegion` → 4 byte LE.
- `reg_read16`/`reg_read8`: idem con length 2 / 1.

**`configure_wifi(x, dev) -> Option<WifiIface>`** (modellata su `configure_msc`):
1. GET_DESCRIPTOR(device, 18 byte) → leggi VID(off 8) / PID(off 10). Se non
   `0x0BDA`/`0x8179` → `None` (non è il nostro device).
2. GET_DESCRIPTOR(config) → walk descrittori; raccogli il **primo** bulk IN e il
   **primo** bulk OUT (attr&0x3==2) della prima interface (RTL8188EU = 1 iface
   vendor-class).
3. `ep_in?`, `ep_out?` (se mancano → None).
4. SET_CONFIGURATION(cfg_val).
5. `reg_read32(REG_SYS_CFG)` → `binfo!("wifi", "rtl8188eu sys_cfg=0x{:08x} (transport ok)")`.
6. ritorna `WifiIface`.

### `kernel/src/usb/registry.rs`
- `SlotKind::Wifi(crate::usb::wifi::WifiState)`.
- arm in `as_str` → `"Wifi"`; arm in `probe_dump` (cfg usb-probe) → `"Wifi"`.

### `kernel/src/usb/device.rs` (dispatch)
Nel blocco `let kind = if dev_class == 0x09 {…} else if configure(...) {…} else if
configure_msc(...) {…}`, aggiungere prima dell'`else` finale:
```rust
} else if let Some(wi) = configure_wifi(x, &mut dev) {
    crate::usb::registry::SlotKind::Wifi(crate::usb::wifi::WifiState {
        iface: wi.iface, ep_in: wi.ep_in, ep_out: wi.ep_out,
        mps_in: wi.mps_in, mps_out: wi.mps_out,
    })
}
```

### `kernel/src/usb/mod.rs`
- `pub mod wifi;`

## Error handling
- VID/PID non match → `None` (passa al ramo successivo; nessun side-effect).
- Config short read / EP bulk mancanti / set_config fail → `None` + eventuale `bwarn!`.
- `reg_read` fallito → log `bwarn!("wifi", "sys_cfg read failed")`, ma il bind
  riesce comunque (lo stato è salvato; SP2 ritenterà).

## Testing
Build-only ora:
1. `make iso` (default) compila — il bind WiFi è sempre compilato (non feature-gated;
   è un driver USB come HID/MSC).
2. `make run-test` invariato (nessun device wifi in QEMU → ramo non preso).
Su HW reale (dopo): boot col dongle → log `usb: dev 0bda:8179 …` poi
`wifi: rtl8188eu sys_cfg=0x******** (transport ok)`.

## File toccati
- kernel/src/usb/wifi/mod.rs (nuovo)
- kernel/src/usb/mod.rs
- kernel/src/usb/registry.rs
- kernel/src/usb/device.rs
- CHANGELOG/358-26-06-08-wifi-sp1-bind-transport.md (nuovo)
