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
            match crate::vfs::fat32::mount_from_ahci_port(port) {
                Ok(()) => crate::binfo!("fat32", "mnt mounted FAT"),
                Err(e) => crate::bwarn!("fat32", "mount /mnt failed: {}", e),
            }
            break;
        }
    }
    Ok(())
}
