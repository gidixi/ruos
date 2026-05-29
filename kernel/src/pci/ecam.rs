//! ECAM addressing + `pci_types::ConfigRegionAccess`. The only kernel-specific
//! config-space code; pci_types does all decoding on top of this.

use alloc::vec::Vec;
use pci_types::{ConfigRegionAccess, PciAddress};
use x86_64::PhysAddr;

use crate::acpi_init::EcamRegion;
use crate::memory::map_io_page; // re-exported from memory::mapper

/// Config-space accessor over the system's ECAM windows.
pub struct EcamAccess {
    regions: Vec<EcamRegion>,
}

impl EcamAccess {
    pub fn new(regions: &[EcamRegion]) -> Self {
        Self { regions: regions.to_vec() }
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Physical address of `(addr, offset)` in ECAM, or `None` if `addr` is not
    /// covered by any region. Per-function config space is 4 KiB at
    /// `base + ((bus - bus_start) << 20 | device << 15 | function << 12)`.
    fn phys(&self, addr: PciAddress, offset: u16) -> Option<PhysAddr> {
        let r = self.regions.iter().find(|r| {
            r.segment == addr.segment()
                && addr.bus() >= r.bus_start
                && addr.bus() <= r.bus_end
        })?;
        let bdf = (u64::from(addr.bus() - r.bus_start) << 20)
            | (u64::from(addr.device()) << 15)
            | (u64::from(addr.function()) << 12);
        Some(PhysAddr::new(r.base + bdf + u64::from(offset & !0x3)))
    }
}

impl ConfigRegionAccess for EcamAccess {
    unsafe fn read(&self, addr: PciAddress, offset: u16) -> u32 {
        let phys = self.phys(addr, offset).expect("ecam: read addr out of range");
        let virt = map_io_page(phys).expect("ecam: map_io_page failed on read");
        unsafe { core::ptr::read_volatile(virt.as_ptr::<u32>()) }
    }

    unsafe fn write(&self, addr: PciAddress, offset: u16, value: u32) {
        let phys = self.phys(addr, offset).expect("ecam: write addr out of range");
        let virt = map_io_page(phys).expect("ecam: map_io_page failed on write");
        unsafe { core::ptr::write_volatile(virt.as_mut_ptr::<u32>(), value) }
    }
}
