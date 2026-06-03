# M2b-1 — Boot-payload availability + ESP file-write (LFN) + copy boot tree — Design

**Date:** 2026-06-03
**Status:** approved (design); LFN-write approach chosen
**Part of:** the ruos SSD self-installer milestone. M1 (GPT read+mount) and M2a
(disk authoring: GPT/FAT write + `mkdisk`) are **done + merged**. This is
**M2b-1 (the copy capability)**; **M2b-2 (the `install` command + target-disk
guards + boot-from-SSD via OVMF)** builds on this and is a separate spec.

## Goal

Make the running ruos able to **reconstruct its own boot tree onto an authored
ESP**, so the target SSD boots standalone. Concretely: ship the kernel ELF +
`BOOTX64.EFI` + `limine.conf` as Limine modules (so they exist in RAM at
runtime), add FAT32 **file write with long-name (LFN)** support to the authoring
writer, and a `copy_boot_payload` routine that writes the full boot tree
(`/EFI/BOOT/BOOTX64.EFI`, `/boot/kernel`, `/boot/limine/limine.conf`, the module
tree `/init.wasm`, `/bin/*.wasm`, `/etc/init.sh`, `/root/*.wasm`) onto the ESP.

M2b-1 is testable standalone: author + copy onto a blank disk in QEMU, extract
the ESP, and assert with `mtools`/`fsck.fat` that the full tree exists with
correct (long) names and byte-identical contents to the ISO source. The actual
boot-from-SSD (UEFI/OVMF) is M2b-2.

## Why LFN write (the chosen approach)

Every one of the 52 userland files uses a 4-character `.wasm` extension, and
`limine.conf` has a 4-character extension — **none are valid 8.3 short names**
(`readdirtest.wasm` also has an 11-char stem). Storing them with the names
Limine/UEFI look for therefore requires either LFN or 8.3-mangling +
regenerating `limine.conf` with the mangled names (and gambling that Limine
finds a renamed config). LFN write is the robust choice: faithful names, copy
`limine.conf` verbatim (its paths already match the ESP layout we replicate), no
mangling, no config-name risk — and it permanently upgrades the FAT writer.

## Background (verified)

- M2a's `kernel/src/vfs/fat32.rs` has a sync, borrow-based `FatWriter<'a>`
  (`open`, `alloc_cluster`, `add_dir_record`, `write_fat_entry`, `mkdir`,
  `find_subdir`) used by `create_dirs`, plus `format`. It is **short-name (8.3)
  only** and has **no file write** (only directory create).
- `kernel/src/disk.rs` `author(dev, esp_mib)` writes GPT + formats ESP+data +
  `create_dirs(["/EFI","/EFI/BOOT"])`. Returns `Layout{esp, data}`.
- `kernel/src/blockdev.rs` `PartBorrow<'a>` (borrowing partition view).
- `kernel/src/modules.rs`: `MODULES: ModulesRequest`; each module has
  `m.data()` (HHDM buffer) and `m.cmdline()` (declared VFS path); `mount_all()`
  copies every module into tmpfs at its cmdline path.
- ISO boot layout (the Makefile): `BOOTX64.EFI` → `/EFI/BOOT/`, kernel →
  `/boot/kernel`, `limine.conf` → `/boot/limine/`, the wasm/init tree at
  `/init.wasm`, `/bin/*.wasm`, `/etc/init.sh`, `/root/*.wasm`. `limine.conf`
  references `path: boot():/boot/kernel` + `module_path: boot():/<vfs path>`.
- Limine UEFI `BOOTX64.EFI` searches for the config in its own dir, then root,
  `/boot/`, `/boot/limine/` — so `/boot/limine/limine.conf` (ISO location) is
  found.

## Architecture — three units

### 1. Boot payload as Limine modules — `limine.conf` + Makefile + `modules.rs`

Add three module entries so the kernel ELF, `BOOTX64.EFI`, and `limine.conf` are
in RAM at runtime (the 52 `.wasm` already are). Use a reserved `/payload/` cmdline
prefix so they are **not** tmpfs-copied (the kernel ELF is multi-MB):

```
    module_path: boot():/boot/kernel
    module_cmdline: /payload/kernel
    module_path: boot():/EFI/BOOT/BOOTX64.EFI
    module_cmdline: /payload/BOOTX64.EFI
    module_path: boot():/boot/limine/limine.conf
    module_cmdline: /payload/limine.conf
```

`modules.rs`:
- `mount_all()` skips cmdlines starting with `/payload/` (no tmpfs copy).
- `pub fn payload(name: &str) -> Option<&'static [u8]>` — find a module whose
  cmdline is `/payload/<name>` and return `m.data()`.
- `pub fn all() -> impl Iterator<Item=(&str /*cmdline*/, &[u8] /*data*/)>` — for
  the copy routine to iterate every module (payload + wasm).

### 2. FAT32 file write + LFN — `kernel/src/vfs/fat32.rs` (`FatWriter`)

Extend `FatWriter` with:
- `write_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), VfsError>`:
  resolve/create parent dirs (reusing the `create_dirs` walk), create the file
  entry, allocate a cluster chain sized for `bytes.len()`, write the bytes
  cluster-by-cluster (zero-padding the last), set the dir record's
  first-cluster + size. Empty file → first cluster 0, size 0 (no allocation).
- **LFN entry generation** (the core new capability): when a name isn't a valid
  8.3 short name (which is all `.wasm` + `limine.conf`), write the long name as N
  preceding 32-byte LFN entries, then the 8.3 short entry:
  - Generate a unique 8.3 **short name** for the directory: uppercase, strip to
    `BASIS` (≤8) + `EXT` (≤3), append `~<n>` numeric tail on collision (scan the
    target dir; n from 1). Set the "lossy" case → standard `~1` form.
  - Compute the **short-name checksum** (the standard `for i in 0..11 { sum =
    ((sum>>1)|(sum<<7)) + name[i] }`).
  - Emit `ceil(len/13)` LFN entries, **last-logical-first physical order**:
    entry k holds UTF-16LE chars [13k..13k+13] at the LFN slots
    (bytes 1..11, 14..26, 28..32), attr `0x0F`, type 0, the checksum, cluster 0;
    the entry with the highest sequence number is OR'd with `0x40` (LAST). The
    name is null-terminated then `0xFFFF`-padded within the final entry.
  - Then the 8.3 short entry (the real file record: first cluster, size, attrs).
  All LFN + short entries go into the parent dir via the existing
  `add_dir_record` slot logic (extended to place a **contiguous run** of K+1
  entries — they must be consecutive; if the current cluster can't fit the run,
  extend the chain and place the whole run in the next cluster).
- `find_subdir`/lookups continue to compare against the generated short or the
  reconstructed long name — for the copy path we only **create**, never need to
  re-find by long name, so a minimal "does this 8.3 short name collide" scan
  suffices (uniqueness, not full LFN read-back).

### 3. Copy the boot tree — `kernel/src/disk.rs` (`copy_boot_payload`)

```
pub fn copy_boot_payload(esp: &mut dyn BlockDevice) -> Result<(), DiskError>
```
- Open a `FatWriter` on the (already-formatted, /EFI/BOOT-containing) ESP.
- Write the three payload files to their UEFI/Limine locations:
  `/EFI/BOOT/BOOTX64.EFI` ← `modules::payload("BOOTX64.EFI")`,
  `/boot/kernel` ← `payload("kernel")`,
  `/boot/limine/limine.conf` ← `payload("limine.conf")` (verbatim).
- Write every wasm/init module to its cmdline path on the ESP:
  for each `(cmdline, data)` in `modules::all()` whose cmdline is NOT `/payload/*`,
  `write_file(cmdline, data)` (e.g. `/bin/ls.wasm`, `/init.wasm`, `/etc/init.sh`,
  `/root/server.wasm`) — matching `limine.conf`'s `module_path boot():<cmdline>`.
- Create intermediate dirs as needed (`/boot`, `/boot/limine`, `/bin`, `/etc`,
  `/root`) via the `write_file` parent-walk.

The trigger (a host fn + the `install` tool wiring) is **M2b-2**; M2b-1 exposes
`copy_boot_payload` and validates it via a test hook that runs author + copy.

## Data flow

```
running ruos (boot payload in RAM as Limine modules)
  └─ disk::author(dev, 64)              → GPT + FAT ESP (/EFI/BOOT) + FAT data
  └─ disk::copy_boot_payload(PartBorrow(ESP))
        ├─ write_file /EFI/BOOT/BOOTX64.EFI  ← payload("BOOTX64.EFI")
        ├─ write_file /boot/kernel           ← payload("kernel")
        ├─ write_file /boot/limine/limine.conf ← payload("limine.conf")
        └─ for each non-payload module: write_file(cmdline, data)  [LFN names]
  ⇒ ESP is a faithful, bootable copy of the ISO boot tree
```

## Error handling

- Missing payload module (`payload(name)` → None) → `DiskError` (the build must
  declare all three; a missing one is a build error surfaced at runtime).
- ESP too small for the payload (kernel + 52 wasm ≈ tens of MB; the 64 MiB ESP
  fits comfortably) → `write_file` cluster alloc returns `NoSpace` → `DiskError`.
- All FAT math checked/saturating (M2a discipline); `write_file` on a
  `PartBorrow` cannot escape the partition.
- LFN run placement: if a directory cluster can't hold the K+1 contiguous
  entries, extend the chain (never split an LFN run across clusters).

## Testing — standalone

`tests/m2b1-test.sh`: boot ruos (with the payload modules) + a blank disk + an
init that runs a test trigger doing `author(dev,64)` then `copy_boot_payload`.
After boot, extract the ESP partition from the image and assert with host tools:
- `mtools` (`mdir -i esp ::/EFI/BOOT`, `::/boot`, `::/bin`) lists the tree with
  **correct long names** (`BOOTX64.EFI`, `kernel`, `limine.conf`, `ls.wasm`,
  `readdirtest.wasm`).
- `fsck.fat -n esp` → clean (LFN entries well-formed: checksums + ordering).
- Byte-identity: `mcopy` a few files OUT of the ESP image (`/boot/kernel`,
  `/bin/ls.wasm`, `/boot/limine/limine.conf`) and `cmp` them against the ISO
  source files — proves the copy is exact.
- `mdir` long-name read-back proves the LFN entries are valid (mtools reads LFN).

Regression: `make run-test` + `make run-gpt-test` + `make run-m2a-test` stay
green (M2b-1 adds new code paths; `format`/`author`/the mount path unchanged).

The reused test trigger: a temporary kernel/tool hook (e.g. extend `mkdisk` with
a `--copy` mode, or a `mkboot` test tool) that runs author + copy on the first
SATA disk — finalized in the plan; M2b-2 replaces it with the real `install`.

## Out of scope → M2b-2

- The `install` command/UX, target-disk **selection + guards** (don't wipe the
  boot medium / a mounted disk), the in-use-port/HBA-reset handling (M2a review
  carry-forwards).
- Booting ruos from the installed SSD (UEFI/OVMF end-to-end).
- LFN **read** in the mounted FAT driver (the `/mnt` path stays short-name; M2b-1
  only adds LFN **write** on the authoring path). Not needed for boot.

## Files touched

- `kernel/src/vfs/fat32.rs` — `FatWriter::write_file` + LFN entry generation +
  contiguous-run `add_dir_record`.
- `kernel/src/modules.rs` — `/payload/` skip in `mount_all`; `payload(name)` +
  `all()` accessors.
- `kernel/src/disk.rs` — `copy_boot_payload`.
- `limine.conf` — 3 payload module entries (kernel, BOOTX64.EFI, limine.conf).
- `Makefile` — ensure the payload files are on the ISO at the module_path
  locations (kernel + BOOTX64.EFI already are; limine.conf already at
  /boot/limine — just add the module declarations).
- `tests/m2b1-test.sh` + a test init + the test trigger hook; CHANGELOG 211.
