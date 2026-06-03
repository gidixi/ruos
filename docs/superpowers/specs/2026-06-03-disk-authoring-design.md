# M2a — Disk-authoring primitives (GPT write + FAT32 mkfs + mkdir) — Design

**Date:** 2026-06-03
**Status:** approved (design)
**Part of:** the ruos SSD self-installer milestone. M1 (read side: GPT parse +
partition mount) is **done + merged** (`a90bb85`). This is **M2a (write side,
primitives)**; **M2b (the installer proper — boot payload as Limine modules +
`install_to` copy + the `install` command + boot-from-SSD)** builds on this and
is a separate spec.

## Goal

Give ruos the ability to **author a disk from scratch**: write a valid GPT, and
format FAT32 partitions with a directory tree — so that a blank SATA SSD becomes
a partitioned, formatted, UEFI-bootable-*shaped* target. The output must be
accepted by real tools (`sgdisk -v`, `fsck.fat`) and re-mountable by ruos's own
M1 reader. Copying the boot payload onto it + making it boot is M2b.

## Why this first (the M2a/M2b split)

M2 is large (7 missing capabilities). Splitting isolates the **write
primitives** — which are independently testable (create a disk in QEMU, verify
its structure on the host + re-mount it) — from the **installer orchestration**
(payload copy, bootloader staging, boot-from-SSD) that depends on them. Mirrors
M1's read-side slice: small, verifiable, mergeable.

## Background (verified by investigation)

- `blockdev::BlockDevice` (read/write_blocks at LBA) + `PartitionDevice`
  (base-LBA offset view) exist (M1). AHCI `write_blocks` works (WRITE DMA EXT,
  bounds-checked, 8192-sector chunking).
- `gpt.rs` is **read-only** (parse header+entries; no CRC validation, no write).
- `vfs/fat32.rs` parses an existing BPB and supports root/parent **file** create
  + write; **`mkdir` is an `Unsupported` stub**; **no mkfs/format**; cannot
  extend a parent dir's cluster chain.
- **No CRC32** anywhere in-tree (no crate dep). GPT headers require CRC32.
- Shell builtins live in `user/shell`; raw-disk work cannot be a pure wasm tool
  (WASI reaches the VFS only, not raw LBA) → primitives are kernel-side.

## Architecture — four units (responsibility-aligned, mirrors M1)

### 1. `kernel/src/crc32.rs` (NEW)

Standard reflected CRC32, polynomial `0xEDB88320` (IEEE 802.3 / the variant GPT
uses). Table-driven:

```
pub fn crc32(data: &[u8]) -> u32           // init 0xFFFFFFFF, reflect, final XOR 0xFFFFFFFF
```

Pure, no_std, host-testable against known vectors (e.g. `crc32(b"123456789") ==
0xCBF43926`). Foundation for the GPT writer.

### 2. `kernel/src/gpt.rs` — add the write side

```
pub struct Extent { pub first_lba: u64, pub sectors: u64 }
pub fn write_layout(dev: &mut dyn BlockDevice, esp_sectors: u64)
        -> Result<(Extent /*esp*/, Extent /*data*/), GptError>
```

Writes a complete GPT for a two-partition layout:
- **Protective MBR** at LBA 0 (one 0xEE partition spanning the disk, capped at
  0xFFFFFFFF sectors per spec; boot signature `55AA`).
- **Primary GPT header** at LBA 1 ("EFI PART", revision `00 00 01 00`, header
  size 92, `my_lba=1`, `alternate_lba=last`, `first_usable`/`last_usable`,
  fresh disk GUID, `partition_entry_lba=2`, `num_entries=128`,
  `size_of_entry=128`, **header CRC32** computed with the CRC field zeroed, and
  **partition-array CRC32**).
- **Partition entry array** at LBA 2 (128×128 = 32 sectors): entry 0 = ESP
  (`TYPE_ESP`, FAT32), entry 1 = Microsoft-Basic-Data (`TYPE_MS_BASIC_DATA`),
  each with a fresh unique GUID, `first_lba`/`last_lba`, name (UTF-16LE
  "EFI System" / "ruos-data"). Remaining entries zero.
- **Backup**: entry-array copy + backup header at the disk tail
  (`alternate_lba`), with `my_lba`/`alternate_lba` swapped and its own CRC.

Layout: ESP starts at LBA 2048 (1 MiB align), `esp_sectors` long; data starts
2048-aligned after the ESP, runs to `last_usable`. Returns the two extents (so
the caller can wrap each in a `PartitionDevice`). Bound/validate disk size
(reject a disk too small for ESP + a data partition).

Unique GUIDs without an RNG-per-call: derive from the existing CSPRNG
(`getrandom`/`rand_chacha` are in-tree) at author time. (A fixed-but-unique
scheme is acceptable for v1; uniqueness across installs is nice-to-have.)

**Also** (CRC now exists): `parse()` gains **CRC validation** — verify the
header CRC32 and the partition-array CRC32; treat a mismatch as "not a valid
GPT" (→ `None` → LBA-0 fallback), same non-fatal contract as M1. Tightens the
read path now that the primitive is available.

### 3. `kernel/src/vfs/fat32.rs` — add `format` + real `mkdir`

**`pub fn format(dev: &mut dyn BlockDevice) -> Result<(), VfsError>`** (mkfs.fat32):
- Compute geometry from `dev.block_count()`: pick `sec_per_cluster` by volume
  size (FAT32 standard thresholds), reserved sectors = 32, 2 FATs, compute
  `fat_sz32` so the FAT covers all data clusters, root cluster = 2.
- Write: boot sector (BPB: jump, OEM, bytes/sec=512, the computed fields, FAT32
  signature `0x29`, volume label, `FAT32   `, boot sig `55AA`); FSInfo sector
  (signatures `0x41615252`/`0x61417272`/`0xAA550000`, free count, next-free);
  backup boot sector (sector 6); zero both FATs then set FAT[0]=media|EOC,
  FAT[1]=EOC, FAT[2]=EOC (root); zero the root-dir cluster.
- Output must pass `fsck.fat` and be mountable by `mtools` + ruos's `from_blockdev`.

**`mkdir`** (replace the `Unsupported` stub in the `FileSystem` impl):
- Resolve/locate the parent dir; alloc a cluster for the new dir; zero it; write
  `.` (points to itself) and `..` (points to parent, 0 if root) entries; add an
  8.3 directory record (ATTR_DIRECTORY) to the parent.
- **Parent-dir-chain-extend helper**: when the parent dir's current cluster is
  full, alloc + link a new cluster into the parent's chain (the missing piece
  the investigator flagged). Needed for `/EFI/BOOT/` and shared with file create.

### 4. `kernel/src/disk.rs` (NEW) — `author` orchestrator + `mkdisk` builtin

```
pub fn author(dev: &mut dyn BlockDevice, esp_mib: u32) -> Result<Layout, DiskError>
```
Ties it together: `gpt::write_layout(dev, esp_mib*…)` → `format(PartitionDevice(ESP))`
→ `mkdir("/EFI")` + `mkdir("/EFI/BOOT")` on the ESP → `format(PartitionDevice(data))`.
Returns the ESP + data extents.

Driven by a **`mkdisk` wasm tool** (`user/mkdisk/`, thin — calls one kernel host
fn `ruos_mkdisk(esp_mib)` that runs `disk::author` on the first SATA port; same
tool+host-fn pattern as every other `/bin` tool, no shell-parser changes). Kept
as a low-level diagnostic; M2b's `install` will *reuse* `disk::author` and add
payload-copy + bootable — no throwaway. `mkdisk` is **destructive** and prints
what it will wipe before proceeding. Raw-disk access stays kernel-side (the wasm
tool only triggers; WASI can't reach raw LBA — absence #6).

## Data flow

```
blank SATA disk
  └─ gpt::write_layout(esp=64 MiB)  → protective MBR + primary/backup GPT (CRCs)
       → (esp_extent, data_extent)
  ├─ PartitionDevice(esp)  → fat32::format → mkdir /EFI → mkdir /EFI/BOOT
  └─ PartitionDevice(data) → fat32::format
  ⇒ partitioned + formatted, empty, UEFI-bootable-shaped target
```

## Error handling

- Disk too small for ESP + data → `DiskError` (non-fatal: `mkdisk` reports it).
- Any block write error → propagate; `mkdisk` reports, leaves the disk partially
  written (acceptable for a destructive diag tool — re-run overwrites).
- All on-disk-structure math bounded; never trust device-reported sizes
  unchecked (overflow-safe, like M1's hardened read path).
- `format` on a `PartitionDevice` cannot escape the partition (M1's clamp).

## Testing — standalone (the point of the split)

`tests/m2a-test.sh`: build the iso (with `INIT_SCRIPT=user-bin/smoke.sh`-style
init that runs `mkdisk` + a round-trip), boot QEMU with a **blank** second AHCI
disk, then verify **two ways**:

1. **Host tools** (spec-conformance — what real UEFI/tools demand): dump the
   disk image and assert `sgdisk -v <img>` → no problems / valid CRCs; the ESP
   FAT passes `fsck.fat -n` and `mtools` lists `/EFI/BOOT`; the data FAT passes
   `fsck.fat -n`.
2. **ruos round-trip** (dogfood): after `mkdisk`, mount the new **data**
   partition with M1's reader, write a file, read it back → serial marker
   `TEST_PASS_M2A`.

CRC32 + (where practical) GPT/format geometry get host unit tests too
(`cargo test` harness, as M1's gpt/blockdev did).

Regression: `make run-test` (raw-FAT `/mnt`) + `make run-gpt-test` (M1 read)
stay green — `format`/`write_layout` are new code paths; existing read/mount
unchanged except `parse()`'s added CRC check (must still accept the test disks,
which are real GPTs with valid CRCs).

## Out of scope → M2b

- Shipping the kernel ELF + `BOOTX64.EFI` + `limine.conf` as Limine modules.
- `install_to`: copying the boot payload onto the new ESP, making it boot.
- The `install` command, target-disk selection (boot medium vs target SSD),
  multi-port enumeration, unmount/re-acquire of an in-use port.
- Booting ruos from the installed SSD with no stick.
- BIOS install (UEFI-only — pure file-copy to the ESP, no `limine bios-install`).

## Files touched

- `kernel/src/crc32.rs` — NEW (CRC32 + host tests).
- `kernel/src/gpt.rs` — add `write_layout` + `Extent`; add CRC validation to `parse`.
- `kernel/src/vfs/fat32.rs` — add `format`; implement `mkdir` + dir-chain-extend.
- `kernel/src/disk.rs` — NEW (`author` orchestrator).
- `kernel/src/main.rs` — `mod crc32; mod disk;`.
- `kernel/src/wasm/host/…` — `ruos_mkdisk(esp_mib)` host fn (kernel-side trigger).
- `user/mkdisk/` — NEW thin wasm tool calling `ruos_mkdisk`; add to `BIN_TOOLS`
  (Makefile) + a `limine.conf` module entry (like every other `/bin` tool).
- `Makefile` / `tests/m2a-test.sh` — GPT+FAT authoring test + changelog.
```
