# 144 — AHCI end-to-end smoke read FAT BPB sector 0

**Data:** 2026-05-29

## Cosa

Smoke test in `boot/phases/storage.rs`: dopo `AhciPort::bringup` di
porta 0, legge sector 0 (FAT BPB) via `read_blocks` e verifica:
- boot signature 0xAA55 a bytes 510..512
- OEM string a bytes 3..11

Log: `disk read OK sector 0 boot_sig=0xaa55 oem="mkfs.fat"`.

Run-test gate aggiunto: `disk read OK sector 0`.

## Perché

Verifica end-to-end pipeline AHCI: BAR map → port bring-up →
IDENTIFY → READ DMA EXT → HHDM phys/virt → caller heap buffer. Sector
0 di un FAT volume formattato da mkfs.fat ha pattern noto, smoke
deterministico senza dipendere da fatfs/FAT parser.

## FAT mount deferred

Provato fatfs (crates.io 0.3 + git tip): 0.3 richiede `core_io` crate
(stable no_std broken), git HEAD richiede log crate moderna che
conflitta con embassy-executor. fatfs mount a /mnt rimandato — task
follow-up: scrivere minimal FAT16/32 reader o trovare crate
alternativo no_std-friendly su stable Rust.

Step 15 MVP rivisto = "AHCI block device usable from kernel" (questo
commit), FAT layer separato.

## Test

`make run-test` → TEST_PASS. Serial:
```
[T+3.907s] INFO ahci HBA up cap=0xc0141f05 vs=0x00010000 ports=6 pi=0x0000003f
[T+3.933s] INFO ahci port 0 sata sectors=131072 model="QEMU HARDDISK"
[T+3.954s] INFO ahci disk read OK sector 0 boot_sig=0xaa55 oem="mkfs.fat"
```

## File toccati

- kernel/src/boot/phases/storage.rs (smoke read sector 0)
- Makefile (gate `disk read OK sector 0`)
- kernel/Cargo.toml (fatfs dep ricomesso)
- CHANGELOG/144-26-05-29-ahci-smoke-read-fat-bpb.md (questo)
