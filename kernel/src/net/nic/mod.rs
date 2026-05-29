//! Real-hardware NIC drivers.
//!
//! Step 14 (`net/virtio.rs`) covers virtio-net inside a hypervisor. This module
//! adds drivers for physical Ethernet controllers (VBox default adapter, bare
//! metal). The probe table maps `(vendor, device)` to a `NicKind`; concrete
//! drivers live in sibling modules and expose `smoltcp::phy::Device`.
//!
//! MVP scope (Task 1-4 of `docs/superpowers/specs/2026-05-29-rust-nic-drivers-design-and-plan.md`):
//! only e1000 is implemented. The enum is sized for the full plan so adding a
//! family later is one variant + one probe-table row.
//!
//! Probe order: the first matching PCI device wins. Loopback always works.

pub mod e1000;
pub mod ring;

use alloc::vec::Vec;
use core::fmt;

/// Errors produced during NIC probe/init.
#[derive(Debug, Clone, Copy)]
pub enum NicError {
    /// PCI enumeration returned no supported NIC.
    NoDevice,
    /// PCI device matched a kind we don't have a driver for yet.
    UnsupportedDevice(u16),
    /// Required BAR (MMIO/IO) missing or zero-sized.
    BarMissing,
    /// Reset / link-up timed out against `timer::ticks()`.
    ResetTimeout,
    /// Link did not come up.
    LinkDown,
    /// DMA region allocation or mapping failed.
    Dma,
}

impl fmt::Display for NicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NicError::NoDevice                 => write!(f, "no supported NIC found"),
            NicError::UnsupportedDevice(id)    => write!(f, "unsupported device 0x{:04x}", id),
            NicError::BarMissing               => write!(f, "BAR missing"),
            NicError::ResetTimeout             => write!(f, "reset/link-up timeout"),
            NicError::LinkDown                 => write!(f, "link down"),
            NicError::Dma                      => write!(f, "DMA alloc/map failed"),
        }
    }
}

/// Driver kind for an enumerated PCI device. New families add a variant here
/// and a row in `PROBE_TABLE`. The plan's full list is enumerated so probe
/// logging logs the *recognised* family even before its driver is implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NicKind {
    E1000,
    E1000e,
    Igb,
    Igc,
    Rtl8139,
    Rtl8169,
    Rtl8125,
    Tg3,
}

impl NicKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            NicKind::E1000   => "e1000",
            NicKind::E1000e  => "e1000e",
            NicKind::Igb     => "igb",
            NicKind::Igc     => "igc",
            NicKind::Rtl8139 => "rtl8139",
            NicKind::Rtl8169 => "rtl8169",
            NicKind::Rtl8125 => "rtl8125",
            NicKind::Tg3     => "tg3",
        }
    }
}

/// `(vendor, device)` → driver. From the spec's Probe table.
/// Order matters only for logging; each PCI device matches at most one row.
const PROBE_TABLE: &[(u16, u16, NicKind)] = &[
    // Intel e1000 — 8254x family
    (0x8086, 0x100E, NicKind::E1000),    // 82540EM (QEMU default)
    (0x8086, 0x1004, NicKind::E1000),
    (0x8086, 0x100F, NicKind::E1000),
    // Intel e1000e — 82574L + ICH/PCH I217/I218/I219 (subset; common ids)
    (0x8086, 0x10D3, NicKind::E1000e),   // 82574L
    (0x8086, 0x153A, NicKind::E1000e),   // I217-LM
    (0x8086, 0x15A0, NicKind::E1000e),   // I218-LM
    (0x8086, 0x15B7, NicKind::E1000e),   // I219-LM
    // Intel igb — I210/I211/I350 + QEMU 82576
    (0x8086, 0x10C9, NicKind::Igb),      // 82576
    (0x8086, 0x150A, NicKind::Igb),      // 82576NS
    (0x8086, 0x1521, NicKind::Igb),      // I350
    (0x8086, 0x1533, NicKind::Igb),      // I210
    (0x8086, 0x1531, NicKind::Igb),      // I210 variants
    (0x8086, 0x1539, NicKind::Igb),      // I211
    // Intel igc — I225/I226
    (0x8086, 0x15F2, NicKind::Igc),
    (0x8086, 0x15F3, NicKind::Igc),
    (0x8086, 0x125B, NicKind::Igc),
    (0x8086, 0x125C, NicKind::Igc),
    // Realtek RTL8139
    (0x10EC, 0x8139, NicKind::Rtl8139),
    // Realtek RTL8169/8168/8111
    (0x10EC, 0x8161, NicKind::Rtl8169),
    (0x10EC, 0x8167, NicKind::Rtl8169),
    (0x10EC, 0x8168, NicKind::Rtl8169),
    (0x10EC, 0x8169, NicKind::Rtl8169),
    (0x10EC, 0x8136, NicKind::Rtl8169),
    // Realtek RTL8125 (2.5G)
    (0x10EC, 0x8125, NicKind::Rtl8125),
    (0x10EC, 0x3000, NicKind::Rtl8125),
    // Broadcom BCM57xx NetXtreme — common ids
    (0x14E4, 0x1677, NicKind::Tg3),      // 5751
    (0x14E4, 0x1681, NicKind::Tg3),      // 5764
    (0x14E4, 0x1692, NicKind::Tg3),      // 57780
];

/// PCI probe + driver kind lookup. Walks `pci::devices()`; returns the first
/// supported `(PciDevice, kind)` or `None`.
fn pci_probe() -> Option<(crate::pci::PciDevice, NicKind)> {
    for dev in crate::pci::devices() {
        for &(v, d, kind) in PROBE_TABLE {
            if dev.vendor_id == v && dev.device_id == d {
                return Some((dev, kind));
            }
        }
    }
    None
}

/// Active NIC after `probe_and_init`. Drivers are added one variant at a time.
/// The enum is kept around even with one variant so adding e1000e/rtl8139/...
/// later is a single-line change (no API churn on `NetState`).
pub enum Nic {
    E1000(e1000::E1000),
}

impl Nic {
    /// Hardware MAC address of the active NIC.
    pub fn mac(&self) -> [u8; 6] {
        match self {
            Nic::E1000(d) => d.mac(),
        }
    }
}

// ─── smoltcp::phy::Device dispatch onto the active variant ──────────────────
//
// Wrapping each driver inside `Nic` keeps NetState polymorphic over the family
// without dyn-trait gymnastics. Each `match` is one arm per variant; we add an
// arm here when we add a variant to the enum.

use smoltcp::phy::{Device, DeviceCapabilities};
use smoltcp::time::Instant;

/// Receive token from one of the family drivers — enum dispatch to keep
/// smoltcp's GAT bounds satisfiable.
pub enum NicRxToken {
    E1000(e1000::E1000RxToken),
}

/// Transmit token analogue.
pub enum NicTxToken<'a> {
    E1000(e1000::E1000TxToken<'a>),
}

impl smoltcp::phy::RxToken for NicRxToken {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, f: F) -> R {
        match self {
            NicRxToken::E1000(t) => t.consume(f),
        }
    }
}

impl<'a> smoltcp::phy::TxToken for NicTxToken<'a> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        match self {
            NicTxToken::E1000(t) => t.consume(len, f),
        }
    }
}

impl Device for Nic {
    type RxToken<'a> = NicRxToken where Self: 'a;
    type TxToken<'a> = NicTxToken<'a> where Self: 'a;

    fn capabilities(&self) -> DeviceCapabilities {
        match self {
            Nic::E1000(d) => d.capabilities(),
        }
    }

    fn receive(&mut self, ts: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        match self {
            Nic::E1000(d) => d.receive(ts)
                .map(|(rx, tx)| (NicRxToken::E1000(rx), NicTxToken::E1000(tx))),
        }
    }

    fn transmit(&mut self, ts: Instant) -> Option<Self::TxToken<'_>> {
        match self {
            Nic::E1000(d) => d.transmit(ts).map(NicTxToken::E1000),
        }
    }
}

/// Walk the probe table and bring up the first supported NIC.
///
/// Logs every recognised match — even families without a driver yet — so the
/// boot serial reports exactly what was seen vs what was bound.
///
/// Task 1 returns `None` unconditionally: drivers wire in starting with Task 3.
pub fn probe_and_init() -> Option<Nic> {
    let (dev, kind) = match pci_probe() {
        Some(x) => x,
        None    => {
            crate::binfo!("nic", "no supported NIC found (probe table miss)");
            return None;
        }
    };
    crate::binfo!(
        "nic",
        "found {:04x}:{:04x} -> {} (bus={} dev={} fn={})",
        dev.vendor_id,
        dev.device_id,
        kind.as_str(),
        dev.address.bus(),
        dev.address.device(),
        dev.address.function(),
    );
    match kind {
        NicKind::E1000 => e1000::E1000::find_and_init().map(Nic::E1000),
        other => {
            crate::bwarn!("nic", "no driver yet for {}", other.as_str());
            None
        }
    }
}
