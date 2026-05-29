//! AHCI driver — SATA storage over a PCIe HBA.
//!
//! Polling-mode, single outstanding command per port. Discovered via
//! `pci::find_class(0x01, 0x06, 0x01)` (Mass Storage / SATA / AHCI).
//! ABAR is BAR5 → MMIO base of the HBA registers; per-port windows live at
//! `ABAR + 0x100 + idx * 0x80`.
//!
//! Public entry point: [`init`] from `boot::phases::storage`. Returns the
//! enumerated [`Hba`] snapshot on success; a kernel-fatal failure logs and
//! returns `None` (storage absent — kernel still boots, /mnt stays unmounted).

pub mod hba;
pub mod port;

pub use hba::{Hba, AhciError};
pub use port::AhciPort;

use spin::Mutex;

/// Global slot for the first usable SATA port. Populated by
/// `boot::phases::storage`; consumed by the FAT mount step (Task 7).
static PORT0: Mutex<Option<AhciPort>> = Mutex::new(None);

pub fn set_port0(p: AhciPort) {
    *PORT0.lock() = Some(p);
}

/// Take the stashed port for moving into the FAT mount.
pub fn take_port0() -> Option<AhciPort> {
    PORT0.lock().take()
}

/// One-shot boot-time AHCI init. Discovers the HBA, resets it, returns the
/// snapshot. Called once from `boot::phases::storage`.
///
/// Returns `None` if no SATA HBA is present in PCI (legitimate on machines
/// without SATA — boot continues, FAT mount is skipped).
pub fn init() -> Option<Hba> {
    match hba::Hba::find_and_init() {
        Ok(hba) => Some(hba),
        Err(AhciError::NotFound) => {
            crate::bwarn!("ahci", "no SATA HBA found — /mnt will be empty");
            None
        }
        Err(e) => {
            crate::bwarn!("ahci", "init failed: {}", e);
            None
        }
    }
}
