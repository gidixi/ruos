# 141 — Task 1: BlockDevice trait skeleton

**Data:** 2026-05-29

## Cosa

`kernel/src/blockdev.rs`: trait `BlockDevice` + `BlockError` enum
+ `Display`. Astrazione storage random-access sector-aligned per
future impl AHCI/NVMe/virtio-blk.

API:
- `block_size() -> u32` (512 su SATA)
- `block_count() -> u64` (LBA48)
- `read_blocks(lba, &mut buf) -> Result<(), BlockError>`
- `write_blocks(lba, &buf) -> Result<(), BlockError>`

Constraint: `buf.len() % block_size() == 0`, caller splits large
transfers (AHCI PRDT cap = 4 MiB per cmd).

`BlockError`: Io, OutOfRange, BadAlignment, Timeout.

Wire `mod blockdev;` in `main.rs`.

## Test

`make build` → `Finished`.

## File toccati

- kernel/src/blockdev.rs (nuovo)
- kernel/src/main.rs (`mod blockdev;`)
- CHANGELOG/141-26-05-29-blockdev-trait.md (questo)
