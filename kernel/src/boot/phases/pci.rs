//! Phase — PCI/PCIe: enumerate ECAM devices, log a smoke summary.
//! Runs after `interrupts` (ACPI + paging are up). Non-fatal if the machine has
//! no ECAM (e.g. a `pc`/i440fx box): logs and continues.

use crate::boot::BootError;
use crate::pci::{Bar, PciError};

pub fn init() -> Result<(), BootError> {
    let acpi = super::get_acpi_info();

    let info = match crate::pci::init(&acpi.ecam) {
        Ok(i) => i,
        Err(PciError::NoEcam) => {
            crate::bwarn!("pci", "no ecam (no PCIe on this machine) — skipping");
            return Ok(());
        }
        Err(e) => {
            return Err(BootError::PciInit(match e {
                PciError::NotInitialized     => "not initialized",
                PciError::AlreadyInitialized => "already initialized",
                PciError::NoEcam             => "no ecam",
            }))
        }
    };

    crate::binfo!("pci", "init ok devices={}", info.device_count);

    if let Some(a) = info.xhci {
        crate::binfo!("pci", "xhci @ {:02x}:{:02x}.{}", a.bus(), a.device(), a.function());
        // Prove BAR0 decode + sizing (xHCI BAR0 is a 64-bit memory BAR).
        if let Some(dev) = crate::pci::find_class(0x0C, 0x03, 0x30) {
            match dev.bar(0) {
                Some(Bar::Memory64 { address, size, .. }) =>
                    crate::binfo!("pci", "xhci bar0=0x{:X} size=0x{:X} (mem64)", address, size),
                Some(Bar::Memory32 { address, size, .. }) =>
                    crate::binfo!("pci", "xhci bar0=0x{:X} size=0x{:X} (mem32)", address, size),
                other => crate::bwarn!("pci", "xhci bar0 unexpected: {:?}", other),
            }
        }
    } else {
        crate::bwarn!("pci", "no xhci found (class 0C/03/30)");
    }

    Ok(())
}
