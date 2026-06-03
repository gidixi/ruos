# M2b-2 — `install` command + target guard + boot-from-SSD — Design

**Date:** 2026-06-03
**Status:** approved (design); auto-target + guard UX chosen
**Part of:** the ruos SSD self-installer milestone — the **capstone**. M1 (GPT
read+mount), M2a (disk authoring + `mkdisk`), M2b-1 (boot-payload copy + `mkboot`)
are done + merged. This adds the safe `install` command, fixes the
port-acquisition carry-forward, and proves the SSD boots standalone.

## Goal

A single safe `install` command that turns a blank SATA SSD into a bootable ruos
system: pick a target disk (guarded so it never wipes the boot medium or a live
`/mnt`), author it (M2a) + copy the boot tree (M2b-1), and have the machine boot
ruos from that SSD with no installer medium. Prove it end-to-end in QEMU under
UEFI (OVMF): boot the ISO, `install` onto a blank disk, then reboot from the disk
(no cdrom) and assert ruos comes up.

## Why now / what's missing

`mkboot` (M2b-1) already authors + copies a bootable tree, but it is an
unguarded diagnostic: it (a) wipes the first SATA port unconditionally, and (b)
re-acquires the port via a **second `ahci::init()` that issues a full HBA reset**
(`hba.rs:95`) — on real hardware that zeroes a live `/mnt`'s PxCLB/PxFB and
orphans its DMA. M2b-2 closes both (the M2a/M2b-1 review carry-forwards) and adds
the boot-from-SSD proof.

## Background (verified)

- `disk::author(dev, esp_mib)` (M2a) + `disk::copy_boot_payload(esp)` (M2b-1) do
  the partition+format+copy. `ruos_mkboot` wires author→PartBorrow(ESP)→copy.
- `kernel/src/ahci/`: `ahci::init()` → `Hba::find_and_init()` does the HBA reset +
  returns `Hba { abar: VirtAddr, pi: u32, … }`. **`AhciPort::bringup(abar, idx)`
  brings up ONE port with NO HBA reset** (just PxCMD/FIS/command-list setup). The
  `abar` MMIO stays mapped for the kernel lifetime. `AhciPort` has `pub sectors:
  u64` + `pub model: String`. There is a `static PORT0` slot but the boot `Hba`
  is currently dropped after `storage::init`.
- `kernel/src/vfs/mod.rs`: `static MOUNTS: Mutex<Vec<(String, FsImpl)>>`; `/mnt`
  is pushed when a FAT mounts. Easy to query.
- OVMF is installed: `/usr/share/OVMF/OVMF_CODE_4M.fd` + `OVMF_VARS_4M.fd` → QEMU
  can boot UEFI from the installed ESP.

## Architecture — three units

### 1. Reset-free port acquisition — `kernel/src/ahci/` (the carry-forward fix)

- Stash the boot HBA on init: a `static BOOT_HBA: Mutex<Option<(VirtAddr, u32)>>`
  (abar, pi) set in `ahci::init()` (or by `storage::init` right after) — so the
  already-reset, already-mapped HBA is reachable post-boot.
- `pub fn acquire_port(idx: usize) -> Option<AhciPort>` = `AhciPort::bringup(abar,
  idx)` using the cached abar — **no `ahci::init()`, no HBA reset**.
- `pub fn sata_ports() -> Vec<usize>` — the populated port indices (from cached
  `pi`, intersect with presence), so `install` can enumerate targets.
- **Refactor `ruos_mkdisk` + `ruos_mkboot`** to acquire their port via
  `acquire_port` instead of `ahci::init()+bringup` — fixing their reset-orphan
  too. (`AhciPort::bringup` must be safe to call on a port the boot brought up +
  dropped; if a stale running port misbehaves, make `bringup` idempotent —
  stop the port `PxCMD.ST/FRE` before reconfiguring. Verify the existing
  mkdisk/mkboot tests still pass after the refactor.)

### 2. `/mnt` guard + `install` — `kernel/src/vfs/mod.rs` + `wasm/host/proc.rs` + `user/install/`

- `vfs::is_mounted(prefix: &str) -> bool` — `MOUNTS.lock().iter().any(|(p,_)| p == prefix)`.
- `ruos_install(esp_mib: i32) -> i32` host fn:
  1. **Guard:** if `vfs::is_mounted("/mnt")` → `bwarn!("install","refusing: /mnt is
     mounted — boot from the installer medium to install")` → return `-3`. (The
     boot medium is the ISO/USB, not SATA, so a SATA target is never the boot
     medium; the live-`/mnt` case is the only dangerous one and this blocks it.)
  2. Enumerate `ahci::sata_ports()`; pick the first → none ⇒ `-1`.
  3. `acquire_port(idx)` (no reset) ⇒ none ⇒ `-1`.
  4. **Log the target before wiping:** `binfo!("install","target: port {} model={:?}
     sectors={} ({} MiB) — WIPING", idx, port.model, port.sectors, port.sectors/2048)`.
  5. `disk::author(&mut port, esp_mib)` ⇒ Err ⇒ `-2`; then `copy_boot_payload`
     over `PartBorrow(layout.esp)` ⇒ Err ⇒ `-2`.
  6. `binfo!("install","ok — ruos installed to port {}, reboot from the SSD", idx)`
     return `0`.
- `user/install/` wasm tool (thin): default `esp_mib=64`; print
  `install: installing ruos to the first SATA disk (ESP {n} MiB)…`; call the
  import; on `0` print `install: ok — remove the installer medium and reboot`;
  on `-3` print `install: /mnt is mounted, refusing (boot the installer medium)`;
  else `install: failed (code N)` + exit 1. Token `install: ok` for the test.
- `mkdisk`/`mkboot` stay as low-level diagnostics; `install` is the safe command.

### 3. Boot-from-SSD proof (OVMF) — `tests/m2b2-test.sh`

Two phases on the same disk image:
- **Phase 1 (install):** boot the ISO (cdrom) + a blank SATA disk +
  `INIT_SCRIPT` running `install`; wait for `install: ok`. The blank disk has no
  GPT ⇒ M1 doesn't mount `/mnt` ⇒ the guard passes ⇒ install authors+copies.
- **Phase 2 (boot from SSD):** boot QEMU **with OVMF (UEFI), NO `-cdrom`**, the
  installed disk as the only drive:
  ```
  cp /usr/share/OVMF/OVMF_VARS_4M.fd build/ovmf_vars.fd   # writable copy
  qemu-system-x86_64 -machine q35 -cpu max \
    -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd \
    -drive if=pflash,format=raw,file=build/ovmf_vars.fd \
    -drive file=build/m2b2-disk.img,format=raw,if=none,id=d0 \
    -device ahci,id=ahci -device ide-hd,drive=d0,bus=ahci.0 \
    -serial stdio -display none -no-reboot -m 1024 -device qemu-xhci
  ```
  UEFI runs `/EFI/BOOT/BOOTX64.EFI` (Limine) from the SSD ESP → reads
  `limine.conf` → loads `/boot/kernel` → ruos boots → M1 mounts `/mnt` from the
  data partition. Assert ruos boot markers on serial (`ruos boot OK` / the shell
  prompt / `mnt mounted FAT`) ⇒ `TEST_PASS_M2B2`.

**The re-install loop is prevented by the guard (important):** `install` copies
the running system's `/etc/init.sh` onto the SSD. In the test that init is the
one that ran `install` in Phase 1, so Phase 2 runs it again — but by then the
boot phases have already mounted `/mnt` from the SSD's own data partition (M1,
before userland/init), so the Phase-2 `install` hits `is_mounted("/mnt")` and
**refuses** (`install: /mnt is mounted`), and boot continues normally. The guard
thus does double duty: safety AND loop-prevention. (In normal interactive use the
running init is a plain shell-boot init, copied verbatim, so the SSD boots to a
shell — no loop either.)

Phase-2 markers to assert: `ruos boot OK` (the installed init ran), `mnt mounted
FAT` / `gpt: data part … -> /mnt` (M1 mounted the SSD's OWN data partition — the
persistence proof), and ideally `install: /mnt is mounted` (the guard fired,
proving loop-prevention). Together they prove the SSD booted ruos standalone.

## Error handling

- `/mnt` mounted ⇒ refuse (`-3`), non-destructive. No SATA ⇒ `-1`. author/copy
  failure ⇒ `-2` (disk left partially written — re-run overwrites; acceptable).
- `acquire_port` must not panic if `BOOT_HBA` is unset (no SATA HBA at boot) ⇒
  `None` ⇒ `-1`.
- All M2a/M2b-1 FAT/GPT safety (checked math, partition isolation, no-panic)
  carries through unchanged.

## Testing

- `make run-m2b2-test` → `TEST_PASS_M2B2` (install + UEFI boot-from-SSD).
- Regression: `run-m2b1-test`, `run-m2a-test`, `run-gpt-test`, `run-test` stay
  green — critically, mkdisk/mkboot must still pass after the `acquire_port`
  refactor (proving reset-free acquisition works).

## Out of scope

- Interactive confirmation prompt / multi-disk index selection (auto-target +
  guard is v1; a `install <idx>`/prompt can come later).
- Re-installing onto the currently-running data disk (refused while `/mnt` is
  mounted; a future `--force` + unmount path).
- BIOS boot (UEFI-only — pure ESP file-copy, no `limine bios-install`). NVMe.
- Stripping the ~20 MB kernel for the installed system (a separate optimisation;
  it fits the 64 MiB ESP today).

## Files touched

- `kernel/src/ahci/mod.rs` (+ `hba.rs`/`port.rs` as needed) — `BOOT_HBA`,
  `acquire_port`, `sata_ports`; idempotent `bringup` if required.
- `kernel/src/boot/phases/storage.rs` — stash the boot HBA.
- `kernel/src/vfs/mod.rs` — `is_mounted`.
- `kernel/src/wasm/host/proc.rs` — `ruos_install`; refactor `ruos_mkdisk`/
  `ruos_mkboot` onto `acquire_port`.
- `user/install/` — NEW thin tool; `Makefile` `BIN_TOOLS` + `limine.conf` entry.
- `tests/m2b2-test.sh` + init; `Makefile` `run-m2b2-test`; CHANGELOG 212.
