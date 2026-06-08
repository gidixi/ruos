//! Realtek RTL8169/8168/8111 driver (PCIe gigabit, device id 10ec:8168).
//!
//! Poll-mode (no IRQ), modelled on `e1000.rs`. Reuses `DescRing`: Realtek
//! descriptors are 16 B, so the ring engine's layout-agnostic slot/buffer/fence
//! machinery applies unchanged. Differences vs e1000 (see design doc
//! 2026-06-08-rtl8169-driver-design.md):
//!   - MMIO lives in the first Memory BAR (BAR2 on 8168), NOT BAR0 (= I/O).
//!   - Descriptor is opts1/opts2 + 64-bit addr; OWN/EOR/FS/LS live in opts1.
//!   - Last ring slot must carry EOR (End Of Ring).
//!   - Config registers are write-locked until 0xC0 is written to 9346CR.
//!   - TX is kicked by writing NPQ to TxPoll (not by advancing a tail register).
//!   - OWN=1 means "owned by NIC" (RX: arm OWN=1, wait for chip to clear;
//!     TX: publish OWN=1, chip clears on completion).

use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;
use x86_64::{PhysAddr, VirtAddr};
use pci_types::Bar;

use crate::net::nic::ring::{DescRing, BUF_SIZE};

const RX_SLOTS: usize = 64;
const TX_SLOTS: usize = 64;
const MTU: usize = 1500;

// Register offsets (RTL8169/8168). Width noted per register.
const REG_IDR0:     usize = 0x00; // MAC, 6 bytes
const REG_TNPDS_LO: usize = 0x20; // TX normal-prio desc start, lo32
const REG_TNPDS_HI: usize = 0x24; // ...hi32
const REG_CR:       usize = 0x37; // Command, u8: RST|RE|TE
const REG_TPPOLL:   usize = 0x38; // TX poll, u8: NPQ
const REG_IMR:      usize = 0x3C; // Interrupt mask, u16
const REG_ISR:      usize = 0x3E; // Interrupt status, u16 (write-1-clear)
const REG_TCR:      usize = 0x40; // TX config, u32
const REG_RCR:      usize = 0x44; // RX config, u32
const REG_9346CR:   usize = 0x50; // EEPROM/config lock, u8: 0xC0 unlock / 0x00 lock
const REG_RMS:      usize = 0xDA; // Max RX packet size, u16
const REG_CPLUSCMD: usize = 0xE0; // C+ command, u16
const REG_RDSAR_LO: usize = 0xE4; // RX desc start, lo32
const REG_RDSAR_HI: usize = 0xE8; // ...hi32

// CR (0x37) bits.
const CR_RST: u8 = 0x10;
const CR_RE:  u8 = 0x08;
const CR_TE:  u8 = 0x04;

// TxPoll (0x38) bits.
const TPPOLL_NPQ: u8 = 0x40;

// 9346CR (0x50) values.
const CFG_UNLOCK: u8 = 0xC0;
const CFG_LOCK:   u8 = 0x00;

// RxConfig (0x44): accept broadcast|multicast|phys-match|all-phys(promisc) +
// RXFTH unlimited (7<<13) + MXDMA unlimited (7<<8).
const RCR_AAP: u32 = 1 << 0; // accept all (promiscuous) — handy for debug
const RCR_APM: u32 = 1 << 1; // accept physical match
const RCR_AM:  u32 = 1 << 2; // accept multicast
const RCR_AB:  u32 = 1 << 3; // accept broadcast
const RCR_RXFTH_UNLIMITED: u32 = 0x7 << 13;
const RCR_MXDMA_UNLIMITED: u32 = 0x7 << 8;

// TxConfig (0x40): MXDMA unlimited (7<<8) + standard IFG (3<<24).
const TCR_MXDMA_UNLIMITED: u32 = 0x7 << 8;
const TCR_IFG_STD: u32 = 0x3 << 24;

// Descriptor opts1 flags.
const OWN: u32 = 1 << 31; // owned by NIC
const EOR: u32 = 1 << 30; // end of ring (last slot)
const FS:  u32 = 1 << 29; // first segment
const LS:  u32 = 1 << 28; // last segment
const FRAME_LEN_MASK: u32 = 0x3FFF; // bits 0:13

/// Realtek 16-byte descriptor (same stride as e1000's; different fields).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct RtlDesc {
    opts1: u32,
    opts2: u32,
    addr:  u64,
}

/// Active RTL8169/8168 NIC.
pub struct Rtl8169 {
    mmio:    VirtAddr,
    rx:      DescRing,
    tx:      DescRing,
    /// Software TX cursor (Realtek has no host-readable TX tail register, so we
    /// track the next slot to publish ourselves; RX reuses `DescRing::head`).
    tx_tail: usize,
    mac:     [u8; 6],
}

impl Rtl8169 {
    /// Probe + initialise the first RTL8169-family device on the PCI bus.
    pub fn find_and_init() -> Option<Self> {
        let dev = crate::pci::devices().into_iter().find(|d| {
            d.vendor_id == 0x10EC
                && matches!(d.device_id, 0x8161 | 0x8167 | 0x8168 | 0x8169 | 0x8136)
        })?;

        dev.enable_mmio();
        dev.enable_bus_master();

        // MMIO is the FIRST Memory BAR — on 8168 that is BAR2 (BAR0 = I/O).
        let (phys, size) = (0..6).find_map(|i| match dev.bar(i) {
            Some(Bar::Memory32 { address, size, .. }) => Some((address as u64, size as u64)),
            Some(Bar::Memory64 { address, size, .. }) => Some((address, size)),
            _ => None,
        })?;
        let virt = crate::memory::mapper::map_io_range(PhysAddr::new(phys), size as usize).ok()?;

        let mut nic = Self {
            mmio:    virt,
            rx:      DescRing::new(RX_SLOTS)?,
            tx:      DescRing::new(TX_SLOTS)?,
            tx_tail: 0,
            mac:     [0; 6],
        };
        nic.init()?;
        crate::binfo!("net", "rtl8169 mac={:02x?}", nic.mac);
        Some(nic)
    }

    pub fn mac(&self) -> [u8; 6] { self.mac }

    fn reg_read8(&self, off: usize) -> u8 {
        unsafe { read_volatile((self.mmio.as_u64() as usize + off) as *const u8) }
    }
    fn reg_write8(&self, off: usize, v: u8) {
        unsafe { write_volatile((self.mmio.as_u64() as usize + off) as *mut u8, v); }
    }
    fn reg_read16(&self, off: usize) -> u16 {
        unsafe { read_volatile((self.mmio.as_u64() as usize + off) as *const u16) }
    }
    fn reg_write16(&self, off: usize, v: u16) {
        unsafe { write_volatile((self.mmio.as_u64() as usize + off) as *mut u16, v); }
    }
    fn reg_write32(&self, off: usize, v: u32) {
        unsafe { write_volatile((self.mmio.as_u64() as usize + off) as *mut u32, v); }
    }

    fn init(&mut self) -> Option<()> {
        // 1. Mask interrupts (poll-mode) + clear status.
        self.reg_write16(REG_IMR, 0);
        self.reg_write16(REG_ISR, 0xFFFF);

        // 2. Soft reset: CR.RST=1, wait clear.
        self.reg_write8(REG_CR, CR_RST);
        let mut ok = false;
        for _ in 0..1_000_000 {
            if (self.reg_read8(REG_CR) & CR_RST) == 0 { ok = true; break; }
            core::hint::spin_loop();
        }
        if !ok { return None; }

        // 3. Unlock config registers.
        self.reg_write8(REG_9346CR, CFG_UNLOCK);

        // 4. Read MAC from IDR0..5.
        for i in 0..6 {
            self.mac[i] = self.reg_read8(REG_IDR0 + i);
        }

        // 5. C+ command: clear inherited offload/VLAN (no-offload MVP).
        self.reg_write16(REG_CPLUSCMD, 0);

        // 6. Initialise RX descriptors: armed (OWN=1), EOR on last slot, buffer
        //    size in the frame-len field, buffer phys in addr.
        for i in 0..RX_SLOTS {
            let eor = if i == RX_SLOTS - 1 { EOR } else { 0 };
            let slot = self.rx.slot(i).as_ptr() as *mut RtlDesc;
            unsafe {
                write_volatile(slot, RtlDesc {
                    opts1: OWN | eor | (BUF_SIZE as u32 & FRAME_LEN_MASK),
                    opts2: 0,
                    addr:  self.rx.buf_phys(i).as_u64(),
                });
            }
        }

        // 7. Initialise TX descriptors: free (OWN=0), EOR on last slot.
        for i in 0..TX_SLOTS {
            let eor = if i == TX_SLOTS - 1 { EOR } else { 0 };
            let slot = self.tx.slot(i).as_ptr() as *mut RtlDesc;
            unsafe {
                write_volatile(slot, RtlDesc {
                    opts1: eor,
                    opts2: 0,
                    addr:  self.tx.buf_phys(i).as_u64(),
                });
            }
        }
        DescRing::release_fence();

        // 8. Program descriptor ring base addresses (256-byte aligned — DMA
        //    regions are page-aligned, so this holds).
        let rdsar = self.rx.desc_phys().as_u64();
        self.reg_write32(REG_RDSAR_LO, rdsar as u32);
        self.reg_write32(REG_RDSAR_HI, (rdsar >> 32) as u32);
        let tnpds = self.tx.desc_phys().as_u64();
        self.reg_write32(REG_TNPDS_LO, tnpds as u32);
        self.reg_write32(REG_TNPDS_HI, (tnpds >> 32) as u32);

        // 9. Max RX packet size = buffer size.
        self.reg_write16(REG_RMS, BUF_SIZE as u16);

        // 10. Enable receiver + transmitter.
        self.reg_write8(REG_CR, CR_TE | CR_RE);

        // 11. RX/TX config (accept B/M/phys/promisc, DMA bursts unlimited).
        self.reg_write32(REG_RCR,
            RCR_AB | RCR_AM | RCR_APM | RCR_AAP | RCR_RXFTH_UNLIMITED | RCR_MXDMA_UNLIMITED);
        self.reg_write32(REG_TCR, TCR_IFG_STD | TCR_MXDMA_UNLIMITED);

        // 12. Lock config registers.
        self.reg_write8(REG_9346CR, CFG_LOCK);

        Some(())
    }

    /// Re-arm RX slot `i` back to the chip: OWN=1, EOR on the last slot, buffer
    /// size in the frame-len field, buffer phys restored.
    fn rearm_rx(&mut self, i: usize, is_last: bool) {
        let eor = if is_last { EOR } else { 0 };
        let slot = self.rx.slot(i).as_ptr() as *mut RtlDesc;
        unsafe {
            write_volatile(slot, RtlDesc {
                opts1: OWN | eor | (BUF_SIZE as u32 & FRAME_LEN_MASK),
                opts2: 0,
                addr:  self.rx.buf_phys(i).as_u64(),
            });
        }
        DescRing::release_fence();
    }
}

// ─── smoltcp tokens + Device impl ───────────────────────────────────────────

pub struct Rtl8169RxToken(Vec<u8>);
pub struct Rtl8169TxToken<'a>(&'a mut Rtl8169);

impl RxToken for Rtl8169RxToken {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, f: F) -> R {
        let mut data = self.0;
        f(&mut data)
    }
}

impl<'a> TxToken for Rtl8169TxToken<'a> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let nic = self.0;
        let tdt = nic.tx_tail;
        let is_last = tdt == nic.tx.count - 1;
        let slot = nic.tx.slot(tdt).as_ptr() as *mut RtlDesc;

        // OWN still set → chip hasn't sent the previous frame in this slot.
        // Queue full: drop (give the closure a throwaway buffer) and let smoltcp
        // retry — same policy as e1000.
        let cur = unsafe { read_volatile(slot) };
        if (cur.opts1 & OWN) != 0 {
            crate::bwarn!("net", "rtl8169: tx queue full, dropping");
            let dummy = nic.tx.buf_virt(tdt).as_mut_ptr::<u8>();
            let buf = unsafe { core::slice::from_raw_parts_mut(dummy, len.min(BUF_SIZE)) };
            return f(buf);
        }

        // Fill the slot's DMA buffer.
        let n = len.min(BUF_SIZE);
        let buf_virt = nic.tx.buf_virt(tdt).as_mut_ptr::<u8>();
        let buf = unsafe { core::slice::from_raw_parts_mut(buf_virt, n) };
        let r = f(buf);

        // Publish: single-segment frame (FS|LS), OWN to chip, EOR on last slot.
        let eor = if is_last { EOR } else { 0 };
        unsafe {
            write_volatile(slot, RtlDesc {
                opts1: OWN | FS | LS | eor | (n as u32 & FRAME_LEN_MASK),
                opts2: 0,
                addr:  nic.tx.buf_phys(tdt).as_u64(),
            });
        }
        DescRing::release_fence();

        // Kick the chip to poll the normal-priority TX queue.
        nic.reg_write8(REG_TPPOLL, TPPOLL_NPQ);

        // Advance the software TX cursor.
        nic.tx_tail = (tdt + 1) % TX_SLOTS;
        r
    }
}

impl Device for Rtl8169 {
    type RxToken<'a> = Rtl8169RxToken where Self: 'a;
    type TxToken<'a> = Rtl8169TxToken<'a> where Self: 'a;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = MTU;
        caps
    }

    fn receive(&mut self, _ts: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let head = self.rx.head();
        let slot = self.rx.slot(head).as_ptr() as *mut RtlDesc;
        DescRing::acquire_fence();
        let desc = unsafe { read_volatile(slot) };

        // OWN still set → chip hasn't filled this slot yet.
        if (desc.opts1 & OWN) != 0 { return None; }

        let is_last = head == self.rx.count - 1;

        // We only support single-buffer frames (FS & LS both set). Otherwise
        // drop and re-arm.
        if (desc.opts1 & FS) == 0 || (desc.opts1 & LS) == 0 {
            self.rearm_rx(head, is_last);
            self.rx.advance_head();
            return None;
        }

        // RX length field includes the 4-byte FCS — strip it.
        let raw = (desc.opts1 & FRAME_LEN_MASK) as usize;
        let len = raw.saturating_sub(4).min(BUF_SIZE);
        let buf_virt = self.rx.buf_virt(head).as_u64() as *const u8;
        let data = unsafe { core::slice::from_raw_parts(buf_virt, len).to_vec() };

        // Re-arm the slot for the chip and advance.
        self.rearm_rx(head, is_last);
        self.rx.advance_head();

        Some((Rtl8169RxToken(data), Rtl8169TxToken(self)))
    }

    fn transmit(&mut self, _ts: Instant) -> Option<Self::TxToken<'_>> {
        Some(Rtl8169TxToken(self))
    }
}
