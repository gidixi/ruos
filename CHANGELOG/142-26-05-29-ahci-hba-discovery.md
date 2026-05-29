# 142 — Task 2: AHCI HBA discovery + reset

**Data:** 2026-05-29

## Cosa

`kernel/src/ahci/` (mod + hba + port stub):
- `hba::Hba::find_and_init()`: `pci::find_class(0x01, 0x06, 0x01)` →
  AHCI HBA, enable MMIO + bus master, map BAR5 (ABAR) via
  `map_io_range`. Reset GHC.HR → bounded wait (~1 s via
  `timer::ticks`). Re-enable GHC.AE post-reset. Mask GHC.IE
  (polling-mode). Snapshot CAP / VS / PI / ABAR.
- `hba::AhciError` { NotFound, BarMissing, ResetTimeout,
  UnsupportedVersion } + Display.
- `port::AhciPort` stub (real bring-up Task 3).
- `kernel/src/ahci/mod.rs::init()`: one-shot entry, returns
  `Option<Hba>` (None se HBA assente — boot continua, /mnt skip).
- `kernel/src/boot/phases/storage.rs`: nuovo phase, chiamato dopo
  fs e prima di userland in `boot::run`.

Wire:
- `mod ahci;` in `main.rs`
- `pub mod storage;` in `boot/phases/mod.rs`
- `phases::storage::init()` in `boot::run`

### Makefile / QEMU

- `DISK_IMG := build/disk.img` (64 MiB raw FAT32 con `hello.txt`
  via `dd` + `mkfs.vfat` + `mcopy`)
- `iso` prereq → `$(DISK_IMG)`
- `run` + `run-test` QEMU args:
  `-boot d` (CRITICO: senza, QEMU tenta boot da disk e hangs silent)
  `-drive file=$(DISK_IMG),format=raw,if=none,id=disk0`
  `-device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0`
- run-test gate aggiunto: `ahci HBA up`

### Dipendenze WSL

- `dosfstools` (mkfs.vfat) + `mtools` (mcopy) installati via apt.

## Test

`make run-test` → TEST_PASS. Serial:
```
[T+4.006s] INFO ahci HBA up cap=0xc0141f05 vs=0x00010000 ports=6 pi=0x0000003f
```

q35 ICH9 AHCI 6-port HBA. Versione 1.0. Reset OK.

## File toccati

- kernel/src/ahci/mod.rs (nuovo)
- kernel/src/ahci/hba.rs (nuovo)
- kernel/src/ahci/port.rs (nuovo, stub)
- kernel/src/boot/phases/storage.rs (nuovo)
- kernel/src/boot/phases/mod.rs (`pub mod storage;`)
- kernel/src/boot/mod.rs (chiama storage)
- kernel/src/main.rs (`mod ahci;`)
- Makefile (DISK_IMG target + -boot d + -drive + -device ahci/ide-hd)
- CHANGELOG/142-26-05-29-ahci-hba-discovery.md (questo)
