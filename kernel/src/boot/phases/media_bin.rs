//! Phase — media_bin: overlay `/bin` off-boot from removable media.
//!
//! The `/bin` tools + app `.cwasm` are NOT Limine modules (off-boot, low RAM);
//! they live on the ISO9660 filesystem of the boot medium and are read on-demand.
//! This phase mounts that filesystem at `/bin`, shadowing the minimal Limine
//! fallback set (shell.wasm + init) already in tmpfs.
//!
//! Two stages, because removable media surface at different times:
//!   1. Pre-userland (this phase): ATAPI CD-ROM (AHCI — ready immediately) and a
//!      short USB-MSC pump (fast path: QEMU/most VMs enumerate the stick at once).
//!   2. Deferred (userland `bin_overlay_task`): real hardware can take seconds to
//!      power + enumerate a USB stick — longer than any reasonable pre-userland
//!      pump. A background task retries the USB-MSC overlay once the executor's
//!      `usb_poll_task` has enumerated the device. Until then the Limine fallback
//!      shell keeps the system usable.

use crate::boot::BootError;
use crate::blockdev::BlockDevice;
use core::sync::atomic::{AtomicBool, Ordering};
use alloc::boxed::Box;

/// Set once `/bin` has been overlaid from any removable medium, so the deferred
/// userland task knows it can stop retrying.
static BIN_OVERLAID: AtomicBool = AtomicBool::new(false);

/// Whether `/bin` has already been overlaid from removable media.
pub fn bin_overlaid() -> bool { BIN_OVERLAID.load(Ordering::Relaxed) }

pub fn init() -> Result<(), BootError> {
    // 1. ATAPI CD-ROM (AHCI). No USB enumeration needed → try first so VM/CD
    //    boots don't pay the USB pump below.
    if atapi_overlay() { return Ok(()); }

    // 2. USB Mass-Storage stick, fast path: pump enumeration briefly. On real
    //    hardware the stick often connects only later — the deferred userland
    //    task (`bin_overlay_task`) covers that; keep this pump short.
    pump_usb();
    log_usb_diag();
    if try_usb_overlay() { return Ok(()); }

    crate::bwarn!("media_bin",
        "no /bin medium yet; Limine fallback active, deferring USB-MSC retry to userland");
    Ok(())
}

/// Drive USB enumeration synchronously for a short window (fast path). Re-seeds
/// connected-but-unenumerated root ports each iteration. Breaks early once a
/// mass-storage slot appears.
fn pump_usb() {
    let end = crate::boot::clock::elapsed_ms() + 3000;
    let mut next_reseed = 0u64;
    loop {
        let now = crate::boot::clock::elapsed_ms();
        if now >= next_reseed {
            crate::usb::reseed_connected_ports();
            next_reseed = now + 250;
        }
        crate::usb::poll();
        if crate::usb::registry::first_msc_slot().is_some() { break; }
        if now >= end { break; }
        core::hint::spin_loop();
    }
}

/// Log a USB device + per-port snapshot — diagnostic for real-hardware boots
/// without serial: tells whether the stick enumerated, as what, and whether any
/// root port even reports a connected/powered device.
fn log_usb_diag() {
    let (total, msc) = crate::usb::registry::slot_summary();
    crate::binfo!("media_bin", "usb enumerated slots={} msc={}", total, msc);
    for (ctrl, slot, port, kind) in crate::usb::registry::dump_slots() {
        crate::binfo!("media_bin", "  usb ctrl={} slot={} port={} kind={}", ctrl, slot, port, kind);
    }
    for (ctrl, port, ccs, ped, pls, pp, speed) in crate::usb::dump_ports() {
        crate::binfo!("media_bin",
            "  ctrl {} port {} ccs={} ped={} pls={} pp={} speed={}", ctrl, port, ccs, ped, pls, pp, speed);
    }
}

/// Mount `/bin` from an ATAPI CD-ROM (ISO9660). True on success.
fn atapi_overlay() -> bool {
    if let Some(cd) = crate::ahci::acquire_atapi_port() {
        match crate::vfs::iso9660::mount_from_blockdev(Box::new(cd), "/bin", "/bin") {
            Ok(()) => {
                BIN_OVERLAID.store(true, Ordering::Relaxed);
                crate::binfo!("media_bin", "/bin overlaid from ISO9660 (ATAPI)");
                return true;
            }
            Err(e) => crate::bwarn!("media_bin", "ATAPI ISO9660 mount failed: {}", e),
        }
    }
    false
}

/// Try to mount `/bin` from a USB Mass-Storage stick (ISO9660 over a 512-byte
/// LUN, scaled to 2048-byte ISO sectors by `SectorScale`). True if `/bin` is (or
/// was already) overlaid. Safe to call repeatedly — used by both this phase and
/// the deferred userland retry task.
pub fn try_usb_overlay() -> bool {
    if BIN_OVERLAID.load(Ordering::Relaxed) { return true; }
    let blk = match crate::usb::msc::first_block() { Some(b) => b, None => return false };
    crate::binfo!("media_bin", "usb-msc: bsize={} blocks={}", blk.block_size(), blk.block_count());
    let dev = Box::new(crate::blockdev::SectorScale::new(Box::new(blk)));
    match crate::vfs::iso9660::mount_from_blockdev(dev, "/bin", "/bin") {
        Ok(()) => {
            BIN_OVERLAID.store(true, Ordering::Relaxed);
            crate::binfo!("media_bin", "/bin overlaid from USB-MSC (ISO9660)");
            true
        }
        Err(e) => {
            crate::bwarn!("media_bin", "USB-MSC ISO9660 mount failed: {}", e);
            false
        }
    }
}
