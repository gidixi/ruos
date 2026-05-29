//! Phase — Storage: discover the AHCI HBA + bring up SATA ports.
//!
//! Runs after PCI and devices, before userland. Non-fatal: machines without
//! SATA log a warning and continue (no `/mnt`).

use crate::boot::BootError;

pub fn init() -> Result<(), BootError> {
    let _hba = crate::ahci::init();
    // Task 2 stops at HBA discovery; per-port bring-up + FAT mount land in
    // Tasks 3-7. Stash the snapshot in a kernel global later when we need it.
    Ok(())
}
