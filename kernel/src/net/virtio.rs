//! virtio-net NIC: discovered via the PCI layer, driven by virtio-drivers,
//! adapted to smoltcp's phy::Device. Polled (no IRQ) by net_poll_task.
//!
//! Design notes:
//! - Discovery: first PCI device with vendor 0x1AF4 and class 0x02 (Network).
//! - Transport: MmioCam over the ECAM HHDM-virtual base + PciTransport.
//! - RX borrow resolution: `receive()` copies the packet out into a Vec<u8>
//!   and immediately calls `recycle_rx_buffer`, so VirtioTxToken can hold
//!   `&mut Inner` without lifetime conflict.  One MTU copy per packet.

use alloc::vec::Vec;
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;
use virtio_drivers::device::net::VirtIONet;
use virtio_drivers::transport::pci::PciTransport;
use virtio_drivers::transport::pci::bus::{Cam, Command, DeviceFunction, MmioCam, PciRoot};

use crate::memory::dma::KernelHal;

/// virtio queue depth — power of two, 16 is fine for a polled driver.
const QUEUE_SIZE: usize = 16;
/// Per-buffer size passed to VirtIONet: must be >= MTU + virtio-net header.
/// 2048 gives 1500 MTU + 12-byte header + slack.
const NET_BUF_LEN: usize = 2048;
/// Reported MTU to smoltcp.
const MTU: usize = 1500;

type Inner = VirtIONet<KernelHal, PciTransport, QUEUE_SIZE>;

/// virtio-net device, ready to be used as a smoltcp `Device`.
pub struct VirtioNet {
    inner: Inner,
    mac:   [u8; 6],
}

impl VirtioNet {
    /// Discover the first virtio-net device on the PCI bus and initialise it.
    /// Returns `None` if no device is present or any initialisation step fails.
    pub fn find_and_init() -> Option<Self> {
        // Locate first virtio NIC: vendor 0x1AF4, base class 0x02 (Network).
        let dev = crate::pci::devices()
            .into_iter()
            .find(|d| d.vendor_id == 0x1AF4 && d.class == 0x02)?;

        // Enable MMIO decode and bus-master DMA on the function.
        dev.enable_mmio();
        dev.enable_bus_master();

        // Build MmioCam over the HHDM virtual base of the ECAM window.
        // SAFETY: `base` is `ecam_phys + HHDM_OFFSET` — the linear HHDM alias
        // of the ECAM physical window.  MmioCam's `cam_offset` for this
        // device_function indexes exactly the 4 KiB config page that was already
        // mapped by `pci::init` via `map_io_page`.  The pointer is valid for
        // at least as long as this function runs (HHDM mappings are permanent).
        let base = crate::pci::ecam_virt_base()?;
        let cam = unsafe { MmioCam::new(base as *mut u8, Cam::Ecam) };
        let mut root = PciRoot::new(cam);

        // Build virtio-drivers DeviceFunction from the PciAddress we discovered.
        let df = DeviceFunction {
            bus:      dev.address.bus(),
            device:   dev.address.device(),
            function: dev.address.function(),
        };

        // Redundantly set Command bits via virtio-drivers so PciTransport::new
        // sees the device ready (our kernel already did this above, but
        // set_command here writes through MmioCam, which is the same page).
        root.set_command(df, Command::MEMORY_SPACE | Command::BUS_MASTER);

        // Build PciTransport — maps BAR regions via H::mmio_phys_to_virt (KernelHal).
        let transport = PciTransport::new::<KernelHal, _>(&mut root, df).ok()?;

        // Initialise the VirtIONet driver (negotiates features, sets up queues).
        let inner = Inner::new(transport, NET_BUF_LEN).ok()?;
        let mac = inner.mac_address();
        crate::binfo!("net", "virtio-net found mac={:02x?}", mac);

        Some(Self { inner, mac })
    }

    /// Return the MAC address as reported by the device.
    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }
}

// ─── smoltcp token types ────────────────────────────────────────────────────

/// Owns a copy of the received packet bytes (after the virtio-net header).
pub struct VirtioRxToken(Vec<u8>);

/// Borrows the inner device mutably so `send` can be called.
pub struct VirtioTxToken<'a>(&'a mut Inner);

impl RxToken for VirtioRxToken {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, f: F) -> R {
        let mut data = self.0;
        f(&mut data)
    }
}

impl<'a> TxToken for VirtioTxToken<'a> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut tx = self.0.new_tx_buffer(len);
        let r = f(tx.packet_mut());
        self.0.send(tx).expect("virtio: send failed");
        r
    }
}

// ─── smoltcp Device impl ────────────────────────────────────────────────────

impl Device for VirtioNet {
    type RxToken<'a> = VirtioRxToken where Self: 'a;
    type TxToken<'a> = VirtioTxToken<'a> where Self: 'a;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = MTU;
        caps
    }

    fn receive(&mut self, _ts: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if !self.inner.can_recv() {
            return None;
        }
        let rx = self.inner.receive().ok()?;
        // Copy the packet out of the RxBuffer so we can immediately recycle it.
        // `rx.packet()` strips the virtio-net header and returns only the
        // Ethernet frame payload slice.
        let data = rx.packet().to_vec();
        // Recycle the buffer slot back into the receive queue.
        self.inner.recycle_rx_buffer(rx).ok()?;
        Some((VirtioRxToken(data), VirtioTxToken(&mut self.inner)))
    }

    fn transmit(&mut self, _ts: Instant) -> Option<Self::TxToken<'_>> {
        if !self.inner.can_send() {
            return None;
        }
        Some(VirtioTxToken(&mut self.inner))
    }
}
