# 352 — Spec: live-CD da USB (driver USB Mass-Storage BOT/SCSI)

**Data:** 2026-06-08

## Cosa
Spec di design per leggere `/bin` off-boot dalla **chiavetta USB di boot** via un
driver USB Mass-Storage (Bulk-Only Transport + subset SCSI read-only), esposto come
`BlockDevice` e montato in ISO9660. Include: infra bulk transfer xHCI, adattatore di
scala settore (2048↔512), nuovo step `media_bin` (sposta l'overlay `/bin` dopo
l'enumerazione USB) e un set minimo Limine (`shell.cwasm` + init) come rete di
sicurezza.

## Perché
Il live-CD ATAPI/ISO9660 funziona in QEMU/VBox ma su HW reale fallisce con
`shell not found`: la ISO è su chiavetta USB (USB Mass-Storage), medium che il
kernel non sa leggere (solo driver USB HID). Servono driver USB-MSC + spostamento
dell'overlay `/bin` dopo l'init USB (oggi `storage` gira prima di `usb`).

## File toccati
- docs/superpowers/specs/2026-06-08-usb-msc-livecd-design.md
- CHANGELOG/352-26-06-08-usb-msc-livecd-spec.md
