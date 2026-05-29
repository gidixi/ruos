//! Phase — Storage: discover the AHCI HBA + bring up SATA ports.
//!
//! Runs after PCI and devices, before userland. Non-fatal: machines without
//! SATA log a warning and continue (no `/mnt`).

use crate::boot::BootError;

pub fn init() -> Result<(), BootError> {
    let hba = match crate::ahci::init() {
        Some(h) => h,
        None    => return Ok(()),
    };

    // Walk Ports-Implemented; bring up every populated SATA port.
    for idx in 0..32 {
        if (hba.pi & (1 << idx)) == 0 { continue; }
        if let Some(port) = crate::ahci::AhciPort::bringup(hba.abar, idx as usize) {
            // Stash the first usable port for the FAT mount phase (Task 7).
            crate::ahci::set_port0(port);
            break;
        }
    }
    Ok(())
}
