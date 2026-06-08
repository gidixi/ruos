# 359 — USB: multi-xHCI (inizializza tutti i controller) + tool rescue nel fallback

**Data:** 2026-06-08

## Cosa
Due cambi emersi dal debug su hardware reale (laptop Tiger Lake):

- **Multi-xHCI** — il kernel inizializzava solo il **primo** controller xHCI per
  indirizzo PCI. Su questo laptop ci sono DUE xHCI: `00:0d.0` (Thunderbolt/USB4,
  porte USB-C vuote) e `00:14.0` (PCH, dove stanno le porte USB-A con la chiavetta
  + tastiera). Prendendo il primo (TBT, vuoto), nessun device veniva mai visto
  (`ccs=false` su tutte le porte). Ora `usb::init` porta su **tutti** gli xHCI:
  - `CTRL` (singleton `Option<Xhci>`) → `CTRLS` (`Vec<Xhci>`); ogni `Xhci` ha `idx`.
  - registro slot namespaced per `(ctrl, slot)` (slot id sono per-controller →
    collidono tra controller); ogni API registry prende il controller.
  - `UsbAction` taggata col controller; `poll` drena gli eventi di ogni controller
    e instrada le azioni al controller giusto.
  - `MscState`/`MscBlock` portano `ctrl`; `dump_ports`/`reseed`/diagnostica iterano
    tutti i controller.
  - dump dei controller USB trovati in PCI (`usb controller xHCI/EHCI ...`).
- **Tool rescue nel fallback** — `limine.conf`: oltre a `shell.wasm`, il set minimo
  carica in RAM `dmesg lspci ls cat echo ip ifconfig ping uname uptime free df ps
  grep head tail wc clear which`. Così, anche senza overlay USB, il sistema è
  **ispezionabile** dalla shell (`dmesg` mostra il log kernel: controller USB,
  tabella porte, mount). Le app GUI grosse restano off-boot.

## Perché
Causa radice del `shell not found`/`/bin` mancante su HW reale: chiavetta sul
secondo xHCI (PCH) mai inizializzato. I tool rescue permettono di diagnosticare
on-device senza seriale.

## File toccati
- kernel/src/usb/xhci/mod.rs
- kernel/src/usb/mod.rs
- kernel/src/usb/registry.rs
- kernel/src/usb/xhci/event.rs
- kernel/src/usb/device.rs
- kernel/src/usb/hub.rs
- kernel/src/usb/msc.rs
- kernel/src/boot/phases/usb.rs
- kernel/src/boot/phases/media_bin.rs
- limine.conf
