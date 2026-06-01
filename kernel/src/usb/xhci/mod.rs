//! xHCI host controller driver.
pub mod regs;

use crate::pci;

/// Spike: find the xHCI controller, map BAR0, read capability registers via the
/// `xhci` crate, and log slot/port counts. Proves the crate + Mapper work.
pub fn init() {
    let dev = match pci::find_class(0x0C, 0x03, 0x30) {
        Some(d) => d,
        None => { crate::bwarn!("usb", "no xhci controller — skipping"); return; }
    };
    dev.enable_mmio();
    dev.enable_bus_master();
    let (base, size) = match dev.bar(0) {
        Some(pci::Bar::Memory64 { address, size, .. }) => (address, size as usize),
        Some(pci::Bar::Memory32 { address, size, .. }) => (address as u64, size as usize),
        other => { crate::bwarn!("usb", "xhci bar0 unexpected: {:?}", other); return; }
    };
    // SAFETY: `base` is the xHCI BAR0 phys; HhdmMapper maps each register block.
    let regs = unsafe {
        xhci::Registers::new(base as usize, regs::HhdmMapper)
    };
    let hcs1 = regs.capability.hcsparams1.read_volatile();
    crate::binfo!("usb", "xhci @ bar0=0x{:X} size=0x{:X} slots={} ports={}",
        base, size, hcs1.number_of_device_slots(), hcs1.number_of_ports());
}

pub fn poll() {}
