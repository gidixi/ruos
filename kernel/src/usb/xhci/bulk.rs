//! Synchronous bulk IN/OUT transfer over an xHCI endpoint transfer ring.
//!
//! Used by the USB Mass-Storage (BOT) driver: enqueue one Normal TRB with IOC,
//! ring the slot doorbell for the endpoint's DCI, then poll for THIS endpoint's
//! Transfer Event (type 32, matching slot + DCI). Mirrors `control.rs`'s
//! `event::wait_for` pattern so foreign events (HID reports, port changes) are
//! dispatched rather than dropped while we wait.

use super::Xhci;
use crate::memory::dma::DmaRegion;

/// Bulk transfer timeout (ms). Flash media occasionally pauses; keep generous.
const BULK_TIMEOUT_MS: u64 = 2000;

/// Push one Normal TRB `(buf_phys, len)` onto `ring`, ring doorbell(slot, dci),
/// and wait for the matching Transfer Event. Returns `(completion_code,
/// residual_bytes)`. `None` on timeout.
pub fn bulk_xfer(
    x: &mut Xhci,
    slot: u8,
    dci: u8,
    ring: &DmaRegion,
    enq: &mut usize,
    cyc: &mut bool,
    buf_phys: u64,
    len: u32,
) -> Option<(u8, u32)> {
    bulk_xfer_timeout(x, slot, dci, ring, enq, cyc, buf_phys, len, BULK_TIMEOUT_MS)
}

/// Like `bulk_xfer` but with a caller-chosen timeout. Used by the WiFi scan,
/// which polls bulk-IN with a SHORT timeout so an idle channel (no queued frame)
/// doesn't stall for the full 2 s per read.
#[allow(clippy::too_many_arguments)]
pub fn bulk_xfer_timeout(
    x: &mut Xhci,
    slot: u8,
    dci: u8,
    ring: &DmaRegion,
    enq: &mut usize,
    cyc: &mut bool,
    buf_phys: u64,
    len: u32,
    timeout_ms: u64,
) -> Option<(u8, u32)> {
    super::ring::enqueue_xfer(ring, enq, cyc, [
        (buf_phys & 0xFFFF_FFFF) as u32,
        (buf_phys >> 32) as u32,
        len,
        (1 << 10) | (1 << 5), // type=1 (Normal) | IOC
    ]);
    x.regs.doorbell.update_volatile_at(slot as usize, |d| {
        d.set_doorbell_target(dci);
    });
    let ev = super::event::wait_for(x, timeout_ms, |w| {
        super::ring::trb_type(w) == 32
            && ((w[3] >> 24) & 0xFF) as u8 == slot
            && ((w[3] >> 16) & 0x1F) as u8 == dci
    })?;
    Some((super::ring::completion_code(&ev) as u8, ev[2] & 0x00FF_FFFF))
}
