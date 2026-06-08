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
pub mod atapi;

pub use hba::{Hba, AhciError};
pub use port::AhciPort;

use spin::Mutex;
use x86_64::VirtAddr;

/// Global slot for the first usable SATA port. Populated by
/// `boot::phases::storage`; consumed by the FAT mount step (Task 7).
static PORT0: Mutex<Option<AhciPort>> = Mutex::new(None);

/// Boot-time HBA snapshot — `(abar, pi)` cached by [`init`] after the one-shot
/// HBA reset. Lets `mkdisk`/`mkboot` bring up a SATA port via the already-reset
/// HBA WITHOUT a second `GHC_HR`, which on real HW would orphan a live `/mnt`.
static BOOT_HBA: Mutex<Option<(VirtAddr, u32)>> = Mutex::new(None); // (abar, pi)

/// Per-port cache of `(model, sectors)` learned from IDENTIFY the first (and,
/// for a mounted port, ONLY) time the port is brought up. `AhciPort::bringup`
/// populates this on every success — including the boot-time bringup of the
/// port that gets mounted at `/mnt`. `disk_info` then lets `disks`
/// (`ruos_sata_list`) report a mounted disk WITHOUT a second `bringup` that
/// would reprogram the live port's PxCLB/PxFB and corrupt its in-flight DMA.
use alloc::string::String;
const NONE_DI: Option<(String, u64)> = None;
static DISK_INFO: Mutex<[Option<(String, u64)>; 32]> = Mutex::new([NONE_DI; 32]);

/// Record port `idx`'s IDENTIFY model + sector count. Called by every
/// successful `AhciPort::bringup` so the info survives the port being moved
/// into a mount (where it can no longer be safely re-queried).
pub fn cache_disk_info(idx: usize, model: String, sectors: u64) {
    if idx < 32 { DISK_INFO.lock()[idx] = Some((model, sectors)); }
}

/// Cached `(model, sectors)` for port `idx`, if it has ever been brought up.
/// `None` means never-seen → safe to `acquire_port` (the port is not mounted).
pub fn disk_info(idx: usize) -> Option<(String, u64)> {
    if idx < 32 { DISK_INFO.lock()[idx].clone() } else { None }
}

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
        Ok(hba) => {
            // Cache the post-reset HBA so mkdisk/mkboot can grab a port without
            // re-issuing the HBA reset (which would orphan a live /mnt's DMA).
            *BOOT_HBA.lock() = Some((hba.abar, hba.pi));
            Some(hba)
        }
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

/// Bring up SATA port `idx` using the boot-time HBA — NO HBA reset, so a live
/// /mnt on another port is not orphaned. None if no HBA recorded or no device.
pub fn acquire_port(idx: usize) -> Option<AhciPort> {
    let (abar, _pi) = (*BOOT_HBA.lock())?;
    AhciPort::bringup(abar, idx)
}

/// Populated SATA port indices from the boot HBA's Ports-Implemented bitmap.
pub fn sata_ports() -> alloc::vec::Vec<usize> {
    let mut v = alloc::vec::Vec::new();
    if let Some((_abar, pi)) = *BOOT_HBA.lock() {
        for idx in 0..32 {
            if pi & (1 << idx) != 0 { v.push(idx); }
        }
    }
    v
}

/// Porta su (bringup) la prima porta ATAPI (CD-ROM) trovata sul boot HBA.
/// `None` se nessuna porta presenta la signature ATAPI. Usato da
/// `boot::phases::storage` per montare `/bin` dal CD live.
pub fn acquire_atapi_port() -> Option<AhciPort> {
    let (abar, pi) = (*BOOT_HBA.lock())?;
    for idx in 0..32 {
        if pi & (1 << idx) == 0 { continue; }
        if let Some(port) = AhciPort::bringup(abar, idx) {
            if port.is_atapi { return Some(port); }
        }
    }
    None
}
