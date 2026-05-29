//! Owned snapshot of one PCI function, built once at enumeration from pci_types.

use pci_types::{Bar, EndpointHeader, PciAddress, PciHeader};

use super::ecam::EcamAccess;

/// One present PCI function. `bars` are decoded (size-probed, 64-bit-aware) at
/// build time; high halves of 64-bit BARs are `None`.
#[derive(Debug, Clone, Copy)]
pub struct PciDevice {
    pub address:   PciAddress,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class:     u8, // base class
    pub subclass:  u8,
    pub prog_if:   u8,
    pub bars:      [Option<Bar>; 6],
}

impl PciDevice {
    /// Build a snapshot for `address` if a function is present there.
    /// Returns `None` for an absent function (vendor id `0xFFFF`).
    pub fn probe(address: PciAddress, access: &EcamAccess) -> Option<Self> {
        let header = PciHeader::new(address);
        let (vendor_id, device_id) = header.id(access);
        if vendor_id == 0xFFFF {
            return None;
        }
        let (_rev, class, subclass, prog_if) = header.revision_and_class(access);

        // BARs only for endpoint (type-0) headers. 64-bit BARs occupy two slots.
        let mut bars = [None; 6];
        if let Some(ep) = EndpointHeader::from_header(header, access) {
            let mut i = 0u8;
            while i < 6 {
                let bar = ep.bar(i, access);
                let is_64 = matches!(bar, Some(Bar::Memory64 { .. }));
                bars[i as usize] = bar;
                i += if is_64 { 2 } else { 1 };
            }
        }

        Some(Self { address, vendor_id, device_id, class, subclass, prog_if, bars })
    }

    /// True if this function advertises multiple functions (header type bit 7).
    pub fn is_multifunction(address: PciAddress, access: &EcamAccess) -> bool {
        PciHeader::new(address).has_multiple_functions(access)
    }
}
