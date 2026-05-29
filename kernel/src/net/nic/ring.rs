//! Shared descriptor-ring engine for NIC drivers.
//!
//! One `DmaRegion` holds N 16-byte descriptors; N separate 2 KiB DMA buffers
//! back the slots. Head/tail indices live in software; the OWN/EOR bits live
//! in the descriptor and are toggled by the NIC ↔ driver. x86 is DMA-coherent,
//! so the ring/buffer memory is normal cacheable RAM — never marked NO_CACHE.
//!
//! The engine owns:
//!   - ring layout (descriptor count, slot stride, phys/virt)
//!   - head/tail indices + wrap math
//!   - the OWN handoff fences (`SeqCst` `compiler_fence` around descriptor
//!     writes paired with the tail-pointer/MMIO write the driver does next)
//!
//! Each driver writes the chip-specific descriptor fields via volatile access
//! into the slot at `desc_virt(i)`. Two descriptor flavours share the same
//! 16-byte slot stride:
//!   - **Legacy** (e1000, RTL8169/8125): `addr u64`, `status u8`, `len u16`,
//!     vendor flags. Single struct.
//!   - **Advanced** (igb/igc): split into a *read* layout (buffer/header
//!     phys) and a *writeback* layout (status/length/checksum). Same 16 B,
//!     different field meaning between the two operations.
//!
//! The engine deliberately does not know either layout — drivers cast
//! `desc_virt(i)` to their own `#[repr(C)]` struct. The shared work here is
//! the index/OWN/EOR/fence dance that's identical across families.

use alloc::vec::Vec;
use core::ptr::NonNull;
use core::sync::atomic::{compiler_fence, Ordering};

use x86_64::{PhysAddr, VirtAddr};

use crate::memory::dma::{alloc as dma_alloc, dealloc as dma_dealloc, DmaRegion};
use crate::memory::frames::PAGE_SIZE;

/// Stride of one descriptor slot. Both legacy and advanced formats are 16 B,
/// so the ring engine can allocate by `N * SLOT_SIZE` and let drivers cast.
pub const SLOT_SIZE: usize = 16;

/// Default per-slot buffer size (2 KiB) — covers a full 1518-byte Ethernet
/// frame with VLAN + FCS slack and aligns nicely inside a 4 KiB DMA page.
pub const BUF_SIZE: usize = 2048;

/// One descriptor ring + its packet buffers.
///
/// Layout in physical memory:
/// ```text
/// desc_region:   [descriptor 0][descriptor 1] ... [descriptor N-1]
///                ^ each 16 B, total N * 16 rounded up to a page.
///
/// buf_regions:   one DmaRegion per slot, each 1 page (4 KiB) holding a
///                BUF_SIZE-byte packet buffer. One-page DMA simplifies
///                the contiguity story (every buffer is naturally aligned
///                and never crosses a page).
/// ```
///
/// `head` is what the driver will read/write next; `tail` is what's been
/// handed off to the chip last. The chip-specific MMIO tail-pointer write
/// (`RDT`/`TDT` on e1000, etc.) belongs to the driver — the ring just keeps
/// the software side honest.
pub struct DescRing {
    /// Backing DMA for the descriptor ring itself.
    pub desc: DmaRegion,
    /// One DMA region per slot (its first BUF_SIZE bytes are the packet buffer).
    pub bufs: Vec<DmaRegion>,
    /// Number of descriptors / slots.
    pub count: usize,
    /// Software head index — next slot the driver will look at.
    head: usize,
}

impl DescRing {
    /// Allocate a ring with `count` slots. `count` should be a power of two for
    /// cheap wrap math, but the engine doesn't enforce that — callers like
    /// e1000 use 64 / 128 / 256.
    pub fn new(count: usize) -> Option<Self> {
        let bytes = count.checked_mul(SLOT_SIZE)?;
        let pages = bytes.div_ceil(PAGE_SIZE as usize);
        let desc  = dma_alloc(pages)?;

        let mut bufs = Vec::with_capacity(count);
        for _ in 0..count {
            match dma_alloc(1) {
                Some(r) => bufs.push(r),
                None    => {
                    // Roll back what we have so far so the caller doesn't leak.
                    for r in bufs.drain(..) { dma_dealloc(r); }
                    dma_dealloc(desc);
                    return None;
                }
            }
        }
        Some(Self { desc, bufs, count, head: 0 })
    }

    /// Physical base of the descriptor ring (what goes in chip's `RDBA`/`TDBA`).
    #[inline] pub fn desc_phys(&self) -> PhysAddr { self.desc.phys }

    /// Virtual base of the descriptor ring (HHDM-mapped).
    #[inline] pub fn desc_virt(&self) -> VirtAddr { self.desc.virt }

    /// Pointer to slot `i` (caller casts to driver-specific struct).
    pub fn slot(&self, i: usize) -> NonNull<u8> {
        let addr = self.desc.virt.as_u64() as usize + i * SLOT_SIZE;
        NonNull::new(addr as *mut u8).expect("ring slot non-null")
    }

    /// Physical address of slot `i`'s packet buffer (what the descriptor's
    /// `buffer_addr` field needs).
    pub fn buf_phys(&self, i: usize) -> PhysAddr { self.bufs[i].phys }

    /// Virtual base of slot `i`'s packet buffer (HHDM).
    pub fn buf_virt(&self, i: usize) -> VirtAddr { self.bufs[i].virt }

    /// Current software head index.
    #[inline] pub fn head(&self) -> usize { self.head }

    /// Bump head one slot forward, wrapping at `count`.
    #[inline] pub fn advance_head(&mut self) {
        self.head = (self.head + 1) % self.count;
    }

    /// Fence to publish a descriptor write *before* the MMIO tail-pointer
    /// store the driver is about to issue. x86 has strong ordering for
    /// normal stores, so a `compiler_fence(SeqCst)` is sufficient — it
    /// pins the compiler from reordering and the architecture guarantees
    /// the rest.
    #[inline] pub fn release_fence() { compiler_fence(Ordering::SeqCst); }

    /// Fence to acquire descriptor data *after* observing a hardware
    /// status flag (e.g. `DD` on e1000 RX). Pairs with the chip writing
    /// the descriptor; same compiler-only barrier on x86.
    #[inline] pub fn acquire_fence() { compiler_fence(Ordering::SeqCst); }
}

impl Drop for DescRing {
    fn drop(&mut self) {
        for r in self.bufs.drain(..) { dma_dealloc(r); }
        dma_dealloc(self.desc);
    }
}
