//! Disk authoring (M2a): GPT + FAT format + dir tree on a raw block device.
//!
//! Ties together the M2a write-side building blocks — `gpt::write_layout`
//! (Task 2), `vfs::fat32::format` (Task 3) and `vfs::fat32::create_dirs`
//! (Task 4/5a) — into a single destructive "lay down a fresh ruos disk"
//! operation. It works from only a borrow of the raw disk: `PartBorrow` carves
//! out each partition region so `format`/`create_dirs` see a clean LBA-0-based
//! view, with the same range checks as the owning `PartitionDevice`.
//!
//! Deliberately NOT routed through `Fat32Fs` / the async `mkdir` / `vfs::mount`:
//! authoring is transient and synchronous and must not touch the global `/mnt`
//! mount table.

use crate::blockdev::{BlockDevice, PartBorrow};
use crate::gpt::Extent;

/// Why `author` could not lay down the disk.
#[derive(Debug)]
pub enum DiskError {
    /// The device is too small (or sized oddly) for the requested layout.
    TooSmall,
    /// A format / dir-tree / block I/O step failed.
    Io,
}

/// The partition placement `author` produced.
pub struct Layout {
    pub esp: Extent,
    pub data: Extent,
}

/// Author a fresh ruos disk on `dev`: a GPT (ESP of `esp_mib` MiB + a data
/// partition filling the rest) with both partitions FAT32-formatted and
/// `/EFI/BOOT` created on the ESP. **Destructive** — overwrites the device.
/// Returns the partition layout.
///
/// Each partition is operated on through a `PartBorrow` whose lifetime ends
/// before the next one begins (the borrows are scoped), so only one partition
/// view of `dev` is live at a time.
pub fn author(dev: &mut dyn BlockDevice, esp_mib: u32) -> Result<Layout, DiskError> {
    let esp_sectors = (esp_mib as u64) * 1024 * 1024 / 512;
    let (esp, data) =
        crate::gpt::write_layout(dev, esp_sectors).map_err(|_| DiskError::TooSmall)?;

    // ESP: format + create the EFI boot dir tree.
    {
        let mut e = PartBorrow::new(dev, esp.first_lba, esp.sectors);
        crate::vfs::fat32::format(&mut e).map_err(|_| DiskError::Io)?;
        crate::vfs::fat32::create_dirs(&mut e, &["/EFI", "/EFI/BOOT"])
            .map_err(|_| DiskError::Io)?;
    }

    // Data partition: just a fresh FAT32 for now.
    {
        let mut d = PartBorrow::new(dev, data.first_lba, data.sectors);
        crate::vfs::fat32::format(&mut d).map_err(|_| DiskError::Io)?;
    }

    Ok(Layout { esp, data })
}
