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

/// Boot-HBA port index consumed by the live-CD `/bin` ISO9660 mount, if the CD
/// sits on the boot HBA (e.g. VirtualBox: single AHCI, CD on port 0). The SATA
/// `/mnt` loop MUST skip this port: a second `bringup` on it would reprogram the
/// live CD port's PxCLB/PxFB and corrupt the in-flight ISO9660 reads.
static BOOT_CD_PORT: Mutex<Option<usize>> = Mutex::new(None);

/// Index of the boot-HBA port owned by the live-CD `/bin` mount, if any.
pub fn boot_cd_port() -> Option<usize> { *BOOT_CD_PORT.lock() }

/// Boot-HBA port index already mounted at `/mnt` (a live FAT SATA disk). Set by
/// `boot::phases::storage` after the FAT mount; read by `acquire_atapi_port` so
/// its ATAPI scan SKIPS this port — a second `bringup` would reprogram the live
/// port's PxCLB/PxFB and corrupt the mounted volume's in-flight DMA. (The ATAPI
/// acquire path is now dormant and no longer used for /bin.)
static MOUNTED_SATA_PORT: Mutex<Option<usize>> = Mutex::new(None);
pub fn set_mounted_sata_port(idx: usize) { *MOUNTED_SATA_PORT.lock() = Some(idx); }

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

/// Bring up the first ATAPI (CD-ROM) port reachable from ANY AHCI controller.
/// First scans the boot HBA (already initialized), then any OTHER AHCI
/// controllers — the live CD can sit on a different HBA than the boot disk
/// (e.g. QEMU q35: CD on the builtin ICH9, disk on an added `-device ahci`).
/// `None` if no ATAPI device is found anywhere. Used by `boot::phases::storage`
/// to mount `/bin` from the live CD.
pub fn acquire_atapi_port() -> Option<AhciPort> {
    let boot = *BOOT_HBA.lock();

    // 1. Boot HBA (already init'd by `init()`) — no re-init / reset. Record the
    //    port index so the SATA /mnt loop skips it (re-bringup would corrupt the
    //    live CD mount — the VirtualBox single-AHCI / CD-on-port-0 case).
    if let Some((abar, pi)) = boot {
        let mounted = *MOUNTED_SATA_PORT.lock();
        for idx in 0..32 {
            if pi & (1 << idx) == 0 { continue; }
            // Skip the port already mounted at /mnt: bringing it up again would
            // corrupt the live FAT volume's DMA (PxCLB/PxFB reprogram).
            if mounted == Some(idx) { continue; }
            if let Some(port) = AhciPort::bringup(abar, idx) {
                if port.is_atapi {
                    *BOOT_CD_PORT.lock() = Some(idx);
                    return Some(port);
                }
            }
        }
    }

    // 2. Any other AHCI controllers (init each; their reset doesn't touch the
    //    boot HBA, so the SATA /mnt bringup that follows is unaffected). A CD on
    //    a different controller can't collide with the boot HBA's SATA loop.
    for h in hba::Hba::find_all_except(boot.map(|(a, _)| a)) {
        if let Some(p) = scan_atapi(h.abar, h.pi) { return Some(p); }
    }
    None
}

/// Bring up every implemented port of one HBA; return the first ATAPI one.
fn scan_atapi(abar: VirtAddr, pi: u32) -> Option<AhciPort> {
    for idx in 0..32 {
        if pi & (1 << idx) == 0 { continue; }
        if let Some(port) = AhciPort::bringup(abar, idx) {
            if port.is_atapi { return Some(port); }
        }
    }
    None
}
