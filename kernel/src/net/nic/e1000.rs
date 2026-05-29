//! Intel e1000 driver (82540EM and friends).
//!
//! MMIO BAR0 driven; poll-mode (no IRQ). One legacy 16-byte RX ring + one
//! legacy 16-byte TX ring, both 64 slots, allocated through `ring::DescRing`.
//!
//! Init sequence (8254x SDM §14.3 / §14.5):
//!   1. PCI bus-master + MMIO decode (caller via `enable_*`).
//!   2. Map BAR0.
//!   3. Reset (CTRL.RST=1 → wait clear).
//!   4. Link up (CTRL.SLU=1, ASDE handled by chip in QEMU).
//!   5. Zero MTA (128 dwords of multicast hash).
//!   6. Read MAC from RAL/RAH.
//!   7. Allocate RX/TX descriptor rings + buffers (`DescRing`).
//!   8. Program RDBAL/RDBAH/RDLEN/RDH/RDT and TDBAL/TDBAH/TDLEN/TDH/TDT.
//!   9. RCTL = EN|BAM|SECRC|BSIZE_2048; TCTL = EN|PSP|CT|COLD; TIPG defaults.
//!
//! Smoltcp `phy::Device` impl:
//!   - `receive`: scan slot at head, if DD bit set → copy bytes to Vec<u8>,
//!     clear status, recycle slot (still owned by us), advance head, bump RDT.
//!   - `transmit`: ensure slot at tail is free → caller fills buffer → set
//!     length, set CMD = EOP|IFCS|RS, bump TDT.

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

// Register offsets (8254x family).
const REG_CTRL:  usize = 0x0000;
const REG_EERD:  usize = 0x0014;
const REG_ICR:   usize = 0x00C0;
const REG_IMS:   usize = 0x00D0;
const REG_IMC:   usize = 0x00D8;
const REG_RCTL:  usize = 0x0100;
const REG_TCTL:  usize = 0x0400;
const REG_TIPG:  usize = 0x0410;
const REG_RDBAL: usize = 0x2800;
const REG_RDBAH: usize = 0x2804;
const REG_RDLEN: usize = 0x2808;
const REG_RDH:   usize = 0x2810;
const REG_RDT:   usize = 0x2818;
const REG_TDBAL: usize = 0x3800;
const REG_TDBAH: usize = 0x3804;
const REG_TDLEN: usize = 0x3808;
const REG_TDH:   usize = 0x3810;
const REG_TDT:   usize = 0x3818;
const REG_MTA:   usize = 0x5200;
const REG_RAL0:  usize = 0x5400;
const REG_RAH0:  usize = 0x5404;

// CTRL bits.
const CTRL_SLU:  u32 = 1 << 6;
const CTRL_RST:  u32 = 1 << 26;

// RCTL bits.
const RCTL_EN:        u32 = 1 << 1;
const RCTL_BAM:       u32 = 1 << 15; // accept broadcast
const RCTL_SECRC:     u32 = 1 << 26; // strip Ethernet CRC
const RCTL_BSIZE_2K:  u32 = 0;       // bits 16:17 = 00 → 2048-byte buffer

// TCTL bits.
const TCTL_EN:    u32 = 1 << 1;
const TCTL_PSP:   u32 = 1 << 3; // pad short packets
const TCTL_CT_DEFAULT:   u32 = 0x10 << 4;  // collision threshold (half-duplex; ignored at gigabit)
const TCTL_COLD_DEFAULT: u32 = 0x40 << 12; // collision distance

// RX descriptor `status` flags.
const RXSTATUS_DD:  u8 = 1 << 0;
const RXSTATUS_EOP: u8 = 1 << 1;

// TX descriptor `cmd` flags.
const TXCMD_EOP:    u8 = 1 << 0;
const TXCMD_IFCS:   u8 = 1 << 1; // insert FCS
const TXCMD_RS:     u8 = 1 << 3; // report status (sets DD on completion)

// TX descriptor `status` flags.
const TXSTATUS_DD: u8 = 1 << 0;

/// Legacy 16-byte RX descriptor.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct RxDesc {
    addr:    u64,
    length:  u16,
    csum:    u16,
    status:  u8,
    errors:  u8,
    special: u16,
}

/// Legacy 16-byte TX descriptor.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct TxDesc {
    addr:    u64,
    length:  u16,
    cso:     u8,
    cmd:     u8,
    status:  u8,
    css:     u8,
    special: u16,
}

/// Active e1000 NIC.
pub struct E1000 {
    /// MMIO virtual base (BAR0 mapped via map_io_range).
    mmio: VirtAddr,
    rx:   DescRing,
    tx:   DescRing,
    mac:  [u8; 6],
}

impl E1000 {
    /// Probe + initialise the first e1000 device on the PCI bus.
    pub fn find_and_init() -> Option<Self> {
        // The shared probe table already mapped this device; we just rediscover
        // it here. Match by vendor 0x8086 + e1000 device ids covered in mod.rs.
        let dev = crate::pci::devices().into_iter().find(|d| {
            d.vendor_id == 0x8086 && matches!(d.device_id, 0x100E | 0x1004 | 0x100F)
        })?;

        dev.enable_mmio();
        dev.enable_bus_master();

        // BAR0 = MMIO base.
        let (phys, size) = match dev.bar(0)? {
            Bar::Memory32 { address, size, .. } => (address as u64, size as u64),
            Bar::Memory64 { address, size, .. } => (address, size),
            Bar::Io { .. } => return None,
        };
        let virt = crate::memory::mapper::map_io_range(PhysAddr::new(phys), size as usize).ok()?;

        let mut nic = Self {
            mmio: virt,
            rx:   DescRing::new(RX_SLOTS)?,
            tx:   DescRing::new(TX_SLOTS)?,
            mac:  [0; 6],
        };
        nic.init()?;
        crate::binfo!("net", "e1000 mac={:02x?}", nic.mac);
        Some(nic)
    }

    pub fn mac(&self) -> [u8; 6] { self.mac }

    fn reg_read(&self, off: usize) -> u32 {
        unsafe { read_volatile((self.mmio.as_u64() as usize + off) as *const u32) }
    }
    fn reg_write(&self, off: usize, v: u32) {
        unsafe { write_volatile((self.mmio.as_u64() as usize + off) as *mut u32, v); }
    }

    fn init(&mut self) -> Option<()> {
        // 1. Mask all interrupts, clear ICR.
        self.reg_write(REG_IMC, 0xFFFF_FFFF);
        let _ = self.reg_read(REG_ICR);

        // 2. Reset.
        let ctrl = self.reg_read(REG_CTRL);
        self.reg_write(REG_CTRL, ctrl | CTRL_RST);
        // Wait for RST to self-clear (8254x: ~1-2 us; bound loops).
        for _ in 0..1_000_000 {
            if (self.reg_read(REG_CTRL) & CTRL_RST) == 0 { break; }
            core::hint::spin_loop();
        }
        if (self.reg_read(REG_CTRL) & CTRL_RST) != 0 { return None; }

        // 3. Mask again post-reset (reset clears IMS but not all chips).
        self.reg_write(REG_IMC, 0xFFFF_FFFF);
        let _ = self.reg_read(REG_ICR);

        // 4. Set link up.
        let ctrl = self.reg_read(REG_CTRL);
        self.reg_write(REG_CTRL, ctrl | CTRL_SLU);

        // 5. Zero MTA (128 dwords).
        for i in 0..128 {
            self.reg_write(REG_MTA + i * 4, 0);
        }

        // 6. Read MAC from RAL/RAH.
        let ral = self.reg_read(REG_RAL0);
        let rah = self.reg_read(REG_RAH0);
        self.mac = [
            ral as u8, (ral >> 8) as u8, (ral >> 16) as u8, (ral >> 24) as u8,
            rah as u8, (rah >> 8) as u8,
        ];

        // 7. Initialise RX descriptors with buffer addresses.
        for i in 0..RX_SLOTS {
            let slot = self.rx.slot(i).as_ptr() as *mut RxDesc;
            unsafe {
                write_volatile(slot, RxDesc {
                    addr:    self.rx.buf_phys(i).as_u64(),
                    length:  0,
                    csum:    0,
                    status:  0,
                    errors:  0,
                    special: 0,
                });
            }
        }

        // 8. Initialise TX descriptors with buffer addresses (length=0, no CMD).
        for i in 0..TX_SLOTS {
            let slot = self.tx.slot(i).as_ptr() as *mut TxDesc;
            unsafe {
                write_volatile(slot, TxDesc {
                    addr:    self.tx.buf_phys(i).as_u64(),
                    length:  0,
                    cso:     0,
                    cmd:     0,
                    status:  TXSTATUS_DD, // mark "complete" so first tx finds it free
                    css:     0,
                    special: 0,
                });
            }
        }

        // 9. Program RX ring registers.
        let rdba = self.rx.desc_phys().as_u64();
        self.reg_write(REG_RDBAL, rdba as u32);
        self.reg_write(REG_RDBAH, (rdba >> 32) as u32);
        self.reg_write(REG_RDLEN, (RX_SLOTS * 16) as u32);
        self.reg_write(REG_RDH, 0);
        self.reg_write(REG_RDT, (RX_SLOTS - 1) as u32); // hw owns [0, RDT-1)

        // 10. Program TX ring registers.
        let tdba = self.tx.desc_phys().as_u64();
        self.reg_write(REG_TDBAL, tdba as u32);
        self.reg_write(REG_TDBAH, (tdba >> 32) as u32);
        self.reg_write(REG_TDLEN, (TX_SLOTS * 16) as u32);
        self.reg_write(REG_TDH, 0);
        self.reg_write(REG_TDT, 0);

        // 11. TIPG defaults for IEEE 802.3 (IPGT=10, IPGR1=8, IPGR2=6).
        self.reg_write(REG_TIPG, 10 | (8 << 10) | (6 << 20));

        // 12. Enable receiver + transmitter.
        self.reg_write(REG_RCTL,
            RCTL_EN | RCTL_BAM | RCTL_SECRC | RCTL_BSIZE_2K);
        self.reg_write(REG_TCTL,
            TCTL_EN | TCTL_PSP | TCTL_CT_DEFAULT | TCTL_COLD_DEFAULT);

        Some(())
    }
}

// ─── smoltcp tokens + Device impl ───────────────────────────────────────────

pub struct E1000RxToken(Vec<u8>);
pub struct E1000TxToken<'a>(&'a mut E1000);

impl RxToken for E1000RxToken {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, f: F) -> R {
        let mut data = self.0;
        f(&mut data)
    }
}

impl<'a> TxToken for E1000TxToken<'a> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let nic = self.0;
        // Find next free TX slot (status.DD set means previous send done).
        let tdt = nic.reg_read(REG_TDT) as usize;
        let slot = nic.tx.slot(tdt).as_ptr() as *mut TxDesc;
        // Hopefully done — if not, drop the packet rather than blocking.
        let st = unsafe { read_volatile(&(*slot).status) };
        if (st & TXSTATUS_DD) == 0 {
            // Queue full; drop and let smoltcp retry.
            crate::bwarn!("net", "e1000: tx queue full, dropping");
            // Caller still needs the closure result.
            let dummy = nic.tx.buf_virt(tdt).as_mut_ptr::<u8>();
            let buf = unsafe { core::slice::from_raw_parts_mut(dummy, len) };
            return f(buf);
        }
        // Hand the closure the slot's DMA buffer (HHDM virt).
        let buf_virt = nic.tx.buf_virt(tdt).as_mut_ptr::<u8>();
        let buf = unsafe { core::slice::from_raw_parts_mut(buf_virt, len.min(BUF_SIZE)) };
        let r = f(buf);
        // Publish the descriptor: length, CMD = EOP|IFCS|RS, clear status.
        unsafe {
            write_volatile(slot, TxDesc {
                addr:    nic.tx.buf_phys(tdt).as_u64(),
                length:  len.min(BUF_SIZE) as u16,
                cso:     0,
                cmd:     TXCMD_EOP | TXCMD_IFCS | TXCMD_RS,
                status:  0,
                css:     0,
                special: 0,
            });
        }
        DescRing::release_fence();
        // Bump TDT to the next slot — chip starts DMA.
        let next = (tdt + 1) % TX_SLOTS;
        nic.reg_write(REG_TDT, next as u32);
        r
    }
}

impl Device for E1000 {
    type RxToken<'a> = E1000RxToken where Self: 'a;
    type TxToken<'a> = E1000TxToken<'a> where Self: 'a;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = MTU;
        caps
    }

    fn receive(&mut self, _ts: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let head = self.rx.head();
        let slot = self.rx.slot(head).as_ptr() as *mut RxDesc;
        DescRing::acquire_fence();
        let desc = unsafe { read_volatile(slot) };
        if (desc.status & RXSTATUS_DD) == 0 { return None; }
        // Single-buffer packet: EOP must also be set (we don't support chained
        // RX descriptors). Anything else → drop and recycle.
        if (desc.status & RXSTATUS_EOP) == 0 {
            unsafe {
                write_volatile(slot, RxDesc {
                    addr: self.rx.buf_phys(head).as_u64(),
                    length: 0, csum: 0, status: 0, errors: 0, special: 0,
                });
            }
            self.recycle_rx(head);
            return None;
        }
        // Copy bytes out of the DMA buffer so smoltcp can mutate them in place.
        let buf_virt = self.rx.buf_virt(head).as_u64() as *const u8;
        let len = (desc.length as usize).min(BUF_SIZE);
        let data = unsafe { core::slice::from_raw_parts(buf_virt, len).to_vec() };

        // Clear status + recycle the slot.
        unsafe {
            write_volatile(slot, RxDesc {
                addr: self.rx.buf_phys(head).as_u64(),
                length: 0, csum: 0, status: 0, errors: 0, special: 0,
            });
        }
        self.recycle_rx(head);

        Some((E1000RxToken(data), E1000TxToken(self)))
    }

    fn transmit(&mut self, _ts: Instant) -> Option<Self::TxToken<'_>> {
        // Always-Some: queue-full handling lives in TxToken::consume.
        Some(E1000TxToken(self))
    }
}

impl E1000 {
    /// Move head forward + bump RDT so the chip can refill the slot.
    fn recycle_rx(&mut self, _slot: usize) {
        let new_rdt = self.rx.head() as u32;
        self.rx.advance_head();
        DescRing::release_fence();
        self.reg_write(REG_RDT, new_rdt);
    }
}
