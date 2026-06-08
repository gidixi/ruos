# 358 — USB-MSC live-CD: fix per hardware reale (port power, fallback, /mnt)

**Data:** 2026-06-08

## Cosa
Fix emersi testando il boot da chiavetta USB su hardware reale (in QEMU non
emergevano):

- **Port Power (PP)** — `usb/xhci/mod.rs`: dopo l'HCRST un controller con Port
  Power Control lascia le root port non alimentate (PORTSC.PP=0) → nessun device
  connette → enumerazione vuota (`slots=0`), chiavetta mai trovata. Ora `init`
  setta PP su tutte le porte (preservando i bit RW1C) con settle ~20ms. QEMU
  auto-alimenta (PP=1) → no-op lì, essenziale su HW reale.
- **Fallback corretto** — `limine.conf`: il set minimo era `shell.cwasm` (shell
  desktop GUI), ma il boot esegue `/bin/shell.wasm` (shell CLI, `disk.rs`
  BOOTSTRAP + executor respawn + SSH). Cambiato a `shell.wasm` → boot a prompt
  garantito anche se l'USB-MSC fallisce.
- **Pump robusto** — `media_bin.rs` + `usb/mod.rs`: re-seed delle porte connesse
  ma non enumerate ogni 250ms (cattura connect tardivi USB3), finestra 8s, +
  diagnostica (`usb enumerated slots=N msc=M` + lista per-slot) per il debug su
  HW senza seriale.
- **Fix regressione /mnt** — `ahci/mod.rs` + `storage.rs`: con l'overlay `/bin`
  spostato in `media_bin` (dopo lo storage), `acquire_atapi_port` ribringup-pava
  la porta SATA già montata a `/mnt` corrompendone la DMA (FAT illeggibile). Ora
  registra la porta `/mnt` (`set_mounted_sata_port`) e la salta nello scan ATAPI.

## Perché
Su HW reale: `slots=0` (PP), `shell.wasm NotFound` (fallback sbagliato). In QEMU
entrambi i gate (`run-test`, `run-test-usb`) restano verdi.

## File toccati
- kernel/src/usb/xhci/mod.rs
- kernel/src/usb/mod.rs
- kernel/src/boot/phases/media_bin.rs
- kernel/src/usb/registry.rs
- kernel/src/ahci/mod.rs
- kernel/src/boot/phases/storage.rs
- limine.conf
