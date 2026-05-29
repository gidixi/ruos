# 140 — Step 15 AHCI + FAT spec + plan

**Data:** 2026-05-29

## Cosa

Spec + plan combinato per Step 15:
`docs/superpowers/specs/2026-05-29-rust-step15-ahci-fat-design.md`.

Obiettivo Step 15: AHCI driver polling (READ + WRITE DMA EXT, LBA48),
trait `BlockDevice`, fatfs no_std come adapter su BlockDevice, mount
FAT a `/mnt` nel VFS esistente, disco persistente via `build/disk.img`
QEMU.

Componenti:
- `kernel/src/blockdev.rs` — trait BlockDevice
- `kernel/src/ahci/{mod, hba, port}.rs` — HBA + per-port engine
- `kernel/src/fs/fatmount.rs` — bridge fatfs ↔ BlockDevice + FatVfs
- `kernel/src/vfs/` — mount table longest-prefix per multi-mount
- `Makefile` — disk.img build via dd + mkfs.vfat + mcopy, QEMU
  `-drive`/`-device ahci`
- Gate run-test: `ahci HBA up`, `ahci port N sata sectors=`,
  `mnt mounted FAT`, `hello from disk`

Scope MVP: read+write FAT a `/mnt`, polling no IRQ, no NCQ,
single-CPU mutex sopra fatfs. Test QEMU q35 `-device ahci`.

Plan 9 task TDD: blockdev trait → AHCI HBA → port bring-up →
IDENTIFY → READ/WRITE DMA EXT → fatfs bridge → VFS mount →
Makefile gate → docs/roadmap done.

## Perché

Persistenza: solo tmpfs RAM finora. Boot WASM `.wasm` ricaricati
ogni volta da initrd Limine. Step 15 = primo storage durabile.

## File toccati

- docs/superpowers/specs/2026-05-29-rust-step15-ahci-fat-design.md (nuovo)
- CHANGELOG/140-26-05-29-step15-ahci-fat-spec.md (questo)
