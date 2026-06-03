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

/// Modules that stay on the ESP as Limine bootstrap (init chain + shell + the
/// network/SSH service). Everything else goes to the data partition (/mnt/bin).
const BOOTSTRAP: &[&str] = &["/init.wasm", "/etc/init.sh", "/bin/shell.wasm",
                             "/root/server.wasm", "/root/client.wasm"];

/// Write the boot tree onto a freshly-authored disk: the bootstrap (+ the slim
/// limine.conf) to the ESP, the command-line tools to the data partition.
///
/// The ESP gets BOOTX64.EFI + kernel + the SLIM limine.conf (which becomes the
/// SSD's `/boot/limine/limine.conf`) + the bootstrap modules, so the SSD boots
/// standalone (UEFI → /EFI/BOOT/BOOTX64.EFI → limine.conf → kernel + init chain).
/// The ~50 `/bin/*.wasm` tools go to the data partition, where they mount at
/// `/mnt/bin` and load on-demand. `author` has already FAT32-formatted BOTH
/// partitions and created /EFI/BOOT on the ESP; `write_file` makes `/bin` (and
/// any other intermediate dirs) on demand.
pub fn copy_boot_payload(dev: &mut dyn crate::blockdev::BlockDevice,
                         layout: &Layout) -> Result<(), DiskError> {
    use crate::vfs::fat32::FatWriter;
    use crate::blockdev::PartBorrow;
    // --- ESP: BOOTX64.EFI + kernel + slim limine.conf + bootstrap modules ---
    {
        let mut esp = PartBorrow::new(dev, layout.esp.first_lba, layout.esp.sectors);
        let mut w = FatWriter::open(&mut esp).map_err(|_| DiskError::Io)?;
        w.write_file("/EFI/BOOT/BOOTX64.EFI",
            crate::modules::payload("BOOTX64.EFI").ok_or(DiskError::Io)?)
            .map_err(|_| DiskError::Io)?;
        w.write_file("/boot/kernel",
            crate::modules::payload("kernel").ok_or(DiskError::Io)?)
            .map_err(|_| DiskError::Io)?;
        // the SLIM config becomes the SSD's limine.conf:
        w.write_file("/boot/limine/limine.conf",
            crate::modules::payload("limine-ssd.conf").ok_or(DiskError::Io)?)
            .map_err(|_| DiskError::Io)?;
        for (cmdline, data) in crate::modules::all() {
            if BOOTSTRAP.contains(&cmdline) {
                w.write_file(cmdline, data).map_err(|_| DiskError::Io)?;
            }
        }
    } // esp PartBorrow dropped — releases the &mut dev borrow
    // --- DATA partition: the /bin/*.wasm tools (mount at /mnt/bin) ---
    {
        let mut d = PartBorrow::new(dev, layout.data.first_lba, layout.data.sectors);
        let mut w = FatWriter::open(&mut d).map_err(|_| DiskError::Io)?;
        for (cmdline, data) in crate::modules::all() {
            if cmdline.starts_with("/payload/") || BOOTSTRAP.contains(&cmdline) { continue; }
            w.write_file(cmdline, data).map_err(|_| DiskError::Io)?; // /bin/ls.wasm → data:/bin/ls.wasm
        }
    }
    Ok(())
}
