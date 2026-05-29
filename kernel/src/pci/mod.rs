//! PCI/PCIe enumeration via ECAM. See docs/superpowers/specs/2026-05-29-rust-pci-ecam-design.md.
//!
//! Discovery only: build a snapshot of every present function, expose lookup +
//! Command-bit helpers. Decoding is done by pci_types over `ecam::EcamAccess`.

pub mod ecam;
mod device;

use alloc::vec::Vec;
use core::fmt;
use spin::{Mutex, Once};

use pci_types::{Bar, CommandRegister, EndpointHeader, PciAddress, PciHeader};

use ecam::EcamAccess;
pub use device::PciDevice;

/// Global PCI state: the live accessor + the device snapshot list. Written once
/// at `init` (single producer). The `Mutex` guards only the Vec publish/clone.
struct PciState {
    access:  EcamAccess,
    devices: Mutex<Vec<PciDevice>>,
}

static PCI: Once<PciState> = Once::new();

#[derive(Debug)]
pub enum PciError {
    NoEcam,
    NotInitialized,
}

impl fmt::Display for PciError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PciError::NoEcam         => f.write_str("no ecam"),
            PciError::NotInitialized => f.write_str("not initialized"),
        }
    }
}

pub struct PciInitInfo {
    pub device_count: usize,
    pub xhci: Option<PciAddress>,
}

/// Enumerate every function on every bus of every ECAM region and publish the
/// `PciDevice` list. `NoEcam` if `regions` is empty (caller decides fatality).
pub fn init(regions: &[crate::acpi_init::EcamRegion]) -> Result<PciInitInfo, PciError> {
    if regions.is_empty() {
        return Err(PciError::NoEcam);
    }
    let access = EcamAccess::new(regions);
    let mut devices: Vec<PciDevice> = Vec::new();

    for r in regions {
        for bus in r.bus_start..=r.bus_end {
            for dev in 0u8..32 {
                let f0 = PciAddress::new(r.segment, bus, dev, 0);
                if let Some(d0) = PciDevice::probe(f0, &access) {
                    let multi = PciDevice::is_multifunction(f0, &access);
                    devices.push(d0);
                    if multi {
                        for func in 1u8..8 {
                            let fa = PciAddress::new(r.segment, bus, dev, func);
                            if let Some(df) = PciDevice::probe(fa, &access) {
                                devices.push(df);
                            }
                        }
                    }
                }
            }
        }
    }

    let device_count = devices.len();
    PCI.call_once(|| PciState { access, devices: Mutex::new(devices) });

    let xhci = find_class(0x0C, 0x03, 0x30).map(|d| d.address);
    Ok(PciInitInfo { device_count, xhci })
}

/// Cloned snapshot of the device list (tiny, write-once). Empty if not inited.
pub fn devices() -> Vec<PciDevice> {
    match PCI.get() {
        Some(s) => s.devices.lock().clone(),
        None => Vec::new(),
    }
}

/// First function matching (class, subclass, prog_if), or `None`.
pub fn find_class(class: u8, subclass: u8, prog_if: u8) -> Option<PciDevice> {
    let s = PCI.get()?;
    s.devices
        .lock()
        .iter()
        .find(|d| d.class == class && d.subclass == subclass && d.prog_if == prog_if)
        .copied()
}

impl PciDevice {
    /// Cached BAR `n` (high half of a 64-bit BAR is `None`).
    pub fn bar(&self, n: usize) -> Option<Bar> {
        self.bars.get(n).copied().flatten()
    }

    /// Set Command Memory-Space bit (required before MMIO BAR access).
    pub fn enable_mmio(&self) {
        self.update_command(|c| c | CommandRegister::MEMORY_ENABLE);
    }

    /// Set Command Bus-Master bit (required before the device can DMA).
    pub fn enable_bus_master(&self) {
        self.update_command(|c| c | CommandRegister::BUS_MASTER_ENABLE);
    }

    fn update_command<F: FnOnce(CommandRegister) -> CommandRegister>(&self, f: F) {
        if let Some(s) = PCI.get() {
            let header = PciHeader::new(self.address);
            if let Some(mut ep) = EndpointHeader::from_header(header, &s.access) {
                ep.update_command(&s.access, f);
            }
        }
    }
}
