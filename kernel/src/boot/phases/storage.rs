//! Phase — Storage: discover the AHCI HBA + bring up SATA ports.
//!
//! Runs after PCI and devices, before userland. Non-fatal: machines without
//! SATA log a warning and continue (no `/mnt`).

use crate::boot::BootError;
use crate::blockdev::BlockDevice;

pub fn init() -> Result<(), BootError> {
    let hba = match crate::ahci::init() {
        Some(h) => h,
        None    => return Ok(()),
    };

    // Walk Ports-Implemented; bring up every populated SATA port.
    for idx in 0..32 {
        if (hba.pi & (1 << idx)) == 0 { continue; }
        if let Some(mut port) = crate::ahci::AhciPort::bringup(hba.abar, idx as usize) {
            // Smoke: read sector 0 (FAT BPB) + confirm 0x55AA boot signature
            // at bytes 510..512. End-to-end proof that READ DMA EXT works
            // against the QEMU disk we formatted with mkfs.vfat.
            let mut buf = alloc::vec![0u8; 512];
            match port.read_blocks(0, &mut buf) {
                Ok(()) => {
                    let sig = u16::from_le_bytes([buf[510], buf[511]]);
                    if sig == 0xAA55 {
                        crate::binfo!(
                            "ahci", "disk read OK sector 0 boot_sig=0x{:04x} oem={:?}",
                            sig,
                            core::str::from_utf8(&buf[3..11]).unwrap_or("?"),
                        );
                    } else {
                        crate::bwarn!("ahci", "sector 0 read but no FAT sig (got 0x{:04x})", sig);
                    }
                }
                Err(e) => crate::bwarn!("ahci", "sector 0 read failed: {}", e),
            }
            // Mount the FAT32 volume at /mnt. Failures log and continue —
            // boot still completes with tmpfs at /.
            //
            // Parse the GPT first: if present, mount the data partition; else
            // fall back to a raw FAT at LBA 0. We copy out (base,count) from the
            // owned GptPartition before moving `port`, so the mutable borrow
            // taken by `parse` has ended by the time we box the port.
            let data_part: Option<(u64, u64)> = crate::gpt::parse(&mut port)
                .and_then(|parts| crate::gpt::find_data(&parts).map(|d| (d.first_lba, d.sectors())));
            let mounted = match data_part {
                Some((base, count)) => {
                    crate::binfo!("storage", "gpt: data part lba={} sectors={} -> /mnt", base, count);
                    let pd = crate::blockdev::PartitionDevice::new(
                        alloc::boxed::Box::new(port), base, count);
                    crate::vfs::fat32::mount_from_blockdev(alloc::boxed::Box::new(pd))
                }
                None => crate::vfs::fat32::mount_from_blockdev(alloc::boxed::Box::new(port)),
            };
            match mounted {
                Ok(())  => crate::binfo!("fat32", "mnt mounted FAT"),
                Err(e)  => crate::bwarn!("fat32", "mount /mnt failed: {}", e),
            }
            break;
        }
    }
    Ok(())
}
