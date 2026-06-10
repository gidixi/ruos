# 404 — Spec: live-CD /bin via archivio compresso bin.bgz caricato da Limine

**Data:** 2026-06-10

## Cosa
Spec di design per popolare `/bin` da un singolo archivio compresso `bin.bgz`
(container di membri gzip indipendenti) caricato da Limine come modulo opaco in
RAM e decompresso dal kernel in tmpfs (nuova fase boot `unpack_bin`). Rimpiazza
il percorso USB-MSC/ATAPI off-boot (changelog 348→359). Decisioni: strategia A
(tutto /bin, wasm+cwasm), mini-fallback rescue se l'archivio manca/è corrotto,
ISO ship solo bin.bgz. Riusa `gzip-core` reso no_std + alloc (feature `std` per
i bin userland).

## Perché
Il live-CD su HW reale richiedeva driver USB-MSC (BOT/SCSI, multi-xHCI,
port-power): complesso e dipendente dall'hardware. Limine legge il medium via
firmware UEFI su ogni HW → caricare un blob compresso e decomprimerlo nel kernel
elimina la dipendenza USB-MSC a runtime e rende la chiavetta scollegabile.

## File toccati
- docs/superpowers/specs/2026-06-10-bin-pack-livecd-design.md
- CHANGELOG/404-26-06-10-bin-pack-livecd-spec.md
