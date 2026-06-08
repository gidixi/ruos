# 353 — Live-CD da USB: driver USB Mass-Storage (BOT/SCSI) + /bin off-boot via media_bin

**Data:** 2026-06-08

## Cosa
Implementato il driver USB Mass-Storage (Bulk-Only Transport + subset SCSI
read-only) e spostato l'overlay `/bin` off-boot dopo l'enumerazione USB, così la
ISO boota a shell anche da chiavetta USB su HW reale (prima: `shell not found`).

- **`usb/xhci/bulk.rs`** (nuovo): bulk transfer sincrono su endpoint ring (Normal
  TRB + doorbell + attesa Transfer Event, pattern di `control.rs`).
- **`usb/msc.rs`** (nuovo): BOT CBW/CSW + CDB SCSI (INQUIRY, TEST UNIT READY,
  READ CAPACITY(10), READ(10), no WRITE); `MscState` (Copy, vive nel registry);
  `MscBlock` che implementa `BlockDevice` copiando lo stato fuori dal registry per
  evitare il deadlock SLOTS durante i transfer; configurazione dei due endpoint
  bulk in un Configure Endpoint command.
- **`usb/device.rs`**: `configure_msc` rileva l'interfaccia BOT (class 0x08 / sub
  0x06 / proto 0x50) + endpoint bulk IN/OUT, fa SET_CONFIGURATION; dispatch in
  `enumerate` → `SlotKind::Msc`.
- **`usb/registry.rs`**: `SlotKind::Msc`, `first_msc_slot`/`msc_state`/
  `set_msc_state`, teardown + probe_dump.
- **`blockdev.rs`**: `SectorScale` — adattatore 2048↔512 per montare ISO9660 su
  LUN USB a 512B senza toccare `iso9660`.
- **`boot/phases/media_bin.rs`** (nuovo): step dopo `usb`; prova ATAPI poi pompa
  l'enumerazione USB e monta `/bin` da USB-MSC. `storage.rs` non monta più `/bin`.
- **`limine.conf`**: `shell.cwasm` ri-aggiunto come modulo (rete di sicurezza in
  RAM se l'overlay USB-MSC fallisce).
- **`Makefile`**: target `run-test-usb` (boot da usb-storage, niente `-cdrom`).

## Perché
Su HW reale la ISO si boota da chiavetta USB (USB Mass-Storage), medium che il
kernel non sapeva leggere (solo USB HID) → `/bin` off-boot vuoto → `shell not
found`. Vedi spec `2026-06-08-usb-msc-livecd-design.md` e changelog 352.

## File toccati
- kernel/src/usb/xhci/bulk.rs
- kernel/src/usb/xhci/mod.rs
- kernel/src/usb/msc.rs
- kernel/src/usb/mod.rs
- kernel/src/usb/device.rs
- kernel/src/usb/registry.rs
- kernel/src/blockdev.rs
- kernel/src/boot/phases/media_bin.rs
- kernel/src/boot/phases/mod.rs
- kernel/src/boot/mod.rs
- kernel/src/boot/phases/storage.rs
- limine.conf
- Makefile
- docs/superpowers/plans/2026-06-08-usb-msc-livecd.md
