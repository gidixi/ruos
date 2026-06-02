# M1 — GPT partition parsing + partition-aware mount (boot-from-SSD persistence) — Design

**Date:** 2026-06-02
**Status:** draft (design), pending review
**Part of:** the ruos SSD self-installer milestone. This is **M1 (read side)**;
**M2 (write side: the self-installer — GPT write + mkfs.fat32 + FAT mkdir +
`install_to` + `install` tool)** builds on this and is a separate spec.

## Goal

Let ruos boot from a GPT-partitioned SATA SSD and mount its **data partition**
(FAT32) read/write as `/mnt`, so user data persists across reboots — instead of
only mounting a raw FAT at LBA 0 (the current QEMU `disk.img` model). This is the
foundation the self-installer (M2) needs (to mount the ESP/data partitions it
creates) and is independently useful: with an externally-prepared GPT disk, ruos
boots from the SSD (via Limine on the ESP) and persists to the data partition.

## Why this first

M2 (the installer) must, after writing a GPT + formatting partitions, **mount
those partitions** to copy files in and to persist. That mount-a-partition
capability is exactly M1. M1 is also testable standalone (prep a GPT disk on the
host, boot ruos, verify it mounts the data partition) — de-risking M2.

## Background (verified)

- `blockdev::BlockDevice` trait: `block_size()`, `block_count()`,
  `read_blocks(lba, buf)`, `write_blocks(lba, buf)`. AHCI ports implement it.
- `vfs::fat32`: mounts on a `Box<dyn BlockDevice + Send>`; reads the BPB from
  **sector 0** of that device (`mount_from_ahci_port` → LBA 0). It has no notion
  of a partition offset.
- `boot/phases/storage.rs`: walks AHCI Ports-Implemented, brings up each SATA
  port, reads sector 0, checks the FAT boot signature, mounts `/mnt`. Assumes the
  FAT begins at LBA 0 (raw-FAT `disk.img`, no partition table).
- No GPT/partition-table code exists.

## Architecture — three small units

### 1. GPT parser — `kernel/src/gpt.rs` (NEW)

Pure read-side parse over a `&mut dyn BlockDevice` (512-byte sectors):

```
struct GptPartition { type_guid: [u8;16], first_lba: u64, last_lba: u64, name: [u16;36] }
fn parse(dev: &mut dyn BlockDevice) -> Option<Vec<GptPartition>>
```

- Read LBA 1 (GPT header). Verify signature `b"EFI PART"` (first 8 bytes). If
  absent → `None` (not GPT; caller falls back to LBA-0 raw FAT).
- From the header: `partition_entry_lba` (u64 @72), `num_partition_entries`
  (u32 @80), `size_of_partition_entry` (u32 @84).
- Read the entry array (num × size bytes, starting at `partition_entry_lba`).
  Each entry: type GUID [0..16], unique GUID [16..32], first_lba (u64 @32),
  last_lba (u64 @40), attrs (u64 @48), name [56..128] (UTF-16LE, 36 chars).
- Skip empty entries (type GUID all-zero). Return the non-empty partitions.
- Bound the entry count (e.g. ≤128) and validate sizes to avoid huge reads on a
  garbage header.

Well-known type GUIDs (mixed-endian, as stored on disk):
- ESP: `C12A7328-F81F-11D2-BA4B-00A0C93EC93B`
- Microsoft Basic Data (our "data" partition): `EBD0A0A2-B9E5-4433-87C0-68B6B72699C7`

A helper `fn is_basic_data(type_guid) -> bool` and `fn is_esp(type_guid) -> bool`
(compare the on-disk byte layout — the first three fields are little-endian).

### 2. Partition offset device — `PartitionDevice` (in `blockdev.rs`)

A `BlockDevice` wrapper that adds a base LBA offset, so the FAT32 driver mounts
on it **unchanged** (it reads "LBA 0" = the partition's first sector):

```
pub struct PartitionDevice { inner: Box<dyn BlockDevice + Send>, base: u64, count: u64 }
impl BlockDevice for PartitionDevice {
    fn block_size(&self)  -> u32  { self.inner.block_size() }
    fn block_count(&self) -> u64  { self.count }                 // partition size
    fn read_blocks(&mut self, lba, buf)  { range-check vs count; inner.read_blocks(self.base + lba, buf) }
    fn write_blocks(&mut self, lba, buf) { range-check vs count; inner.write_blocks(self.base + lba, buf) }
}
```

`count = last_lba - first_lba + 1`; reads/writes past it → `BlockError::OutOfRange`.

### 3. GPT-aware storage phase — `boot/phases/storage.rs` (MODIFY)

For each populated SATA port:
1. `gpt::parse(&mut port)`:
   - **Some(parts)** → find the partition to mount as `/mnt`: the first
     **Microsoft Basic Data** partition (the data partition; the ESP is skipped —
     Limine, not ruos, reads the ESP). Wrap it in a `PartitionDevice {base:
     first_lba, count}` and mount FAT32 on that. Log `storage: gpt data part
     lba=.. mounted /mnt`.
   - **None** → fall back to the current behavior: mount FAT32 on the whole
     device at LBA 0 (the raw-FAT `disk.img`). Log as today.
2. Mount the first device that yields a valid FAT (data partition or raw),
   exactly as today (single `/mnt`).

`vfs::fat32` gains a `mount_from_blockdev(dev: Box<dyn BlockDevice + Send>)`
(generalising `mount_from_ahci_port`, which becomes a thin wrapper). No change to
the FAT logic itself — the offset lives entirely in `PartitionDevice`.

## Data flow

```
AHCI port (BlockDevice)
   └─ gpt::parse → [ESP, data, …]?
        ├─ yes → pick Microsoft-Basic-Data → PartitionDevice{base,count}
        │         └─ fat32::mount_from_blockdev → /mnt  (offsets all I/O by base)
        └─ no  → fat32::mount_from_blockdev(whole device) → /mnt  (LBA 0, as today)
```

## Error handling

- No GPT signature → fall back to LBA-0 (non-fatal; preserves current behavior).
- GPT present but no Basic-Data partition → log + fall back to LBA-0 / skip
  (don't mount the ESP as /mnt). Non-fatal: boot continues, `/mnt` just absent.
- Malformed GPT (bad entry count/size, short reads) → treat as "no GPT", fall
  back. Bound all reads; never trust header fields unchecked.
- `PartitionDevice` out-of-range → `BlockError::OutOfRange` (the FAT driver
  already maps block errors to `VfsError`).

## Testing

1. **GPT data-partition mount** (new): build a GPT test disk on the host —
   `sgdisk` an ESP (EF00) + a Microsoft-Basic-Data partition (0700), `mkfs.vfat`
   the data partition, drop a marker file `gpt-hello.txt`. Boot QEMU with it as a
   second AHCI disk; assert a boot-log marker `storage: gpt data part … mounted`
   and that `cat /mnt/gpt-hello.txt` returns the marker (extend `smoke.sh` /
   a `run-gpt-test`).
2. **Raw-FAT fallback** (regression): the existing `disk.img` (raw FAT at LBA 0)
   still mounts — current `make run-test` markers (`mnt mounted FAT`,
   `hello from disk`) stay green.
3. **VBox / real**: a GPT-partitioned SATA disk mounts + persists across reboot.

## Out of scope (→ M2, the installer)

- Writing a GPT, mkfs.fat32, FAT `mkdir`, copying the bootloader/kernel/modules,
  the `install` tool, source-files-as-Limine-modules. M1 is **read + mount only**.
- Mounting the ESP at runtime (Limine handles boot; ruos needn't touch the ESP).
- NVMe (SATA/AHCI only). Multiple data partitions / non-FAT filesystems.

## Files touched

- `kernel/src/gpt.rs` — NEW (GPT header + entry parse, type-GUID helpers).
- `kernel/src/blockdev.rs` — add `PartitionDevice` wrapper.
- `kernel/src/vfs/fat32.rs` — `mount_from_blockdev(Box<dyn BlockDevice+Send>)`;
  `mount_from_ahci_port` becomes a thin wrapper.
- `kernel/src/boot/phases/storage.rs` — GPT-parse, pick data partition, mount via
  `PartitionDevice`; LBA-0 fallback.
- `kernel/src/main.rs` — `mod gpt;`.
- `Makefile` / `tests/` — GPT test disk + `run-gpt-test`; changelog.
