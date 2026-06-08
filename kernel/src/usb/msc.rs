//! USB Mass-Storage class driver — Bulk-Only Transport (BOT) + a read-only SCSI
//! subset, exposed as a `BlockDevice` so the live-CD `/bin` can be read on-demand
//! from the boot USB stick (real hardware boots the ISO from USB, not an ATAPI
//! CD-ROM). See docs/superpowers/specs/2026-06-08-usb-msc-livecd-design.md.
//!
//! Scope: read-only. INQUIRY / TEST UNIT READY / READ CAPACITY(10) / READ(10).
//! No WRITE, no UAS (every flash drive also exposes a BOT interface).

use crate::usb::xhci::Xhci;
use crate::usb::device::UsbDevice;
use crate::memory::dma::DmaRegion;
use crate::blockdev::{BlockDevice, BlockError};

// ── Bulk-Only Transport wire format (pure, unit-tested below) ───────────────

pub const CBW_LEN: usize = 31;
pub const CSW_LEN: usize = 13;

/// Parsed Command Status Wrapper.
pub struct Csw { pub tag: u32, pub residue: u32, pub status: u8 }

/// Build a 31-byte Command Block Wrapper. `data_in` true = device→host.
pub fn build_cbw(tag: u32, data_len: u32, data_in: bool, lun: u8, cdb: &[u8]) -> [u8; CBW_LEN] {
    let mut b = [0u8; CBW_LEN];
    b[0..4].copy_from_slice(b"USBC");
    b[4..8].copy_from_slice(&tag.to_le_bytes());
    b[8..12].copy_from_slice(&data_len.to_le_bytes());
    b[12] = if data_in { 0x80 } else { 0x00 };
    b[13] = lun & 0x0F;
    b[14] = cdb.len() as u8;
    b[15..15 + cdb.len()].copy_from_slice(cdb);
    b
}

/// Parse + validate a Command Status Wrapper against the expected tag.
pub fn parse_csw(b: &[u8], tag: u32) -> Option<Csw> {
    if b.len() < CSW_LEN || &b[0..4] != b"USBS" { return None; }
    let got = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    if got != tag { return None; }
    Some(Csw {
        tag: got,
        residue: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
        status: b[12],
    })
}

/// SCSI READ(10) CDB.
pub fn scsi_read10(lba: u32, blocks: u16) -> [u8; 10] {
    let mut c = [0u8; 10];
    c[0] = 0x28;
    c[2..6].copy_from_slice(&lba.to_be_bytes());
    c[7..9].copy_from_slice(&blocks.to_be_bytes());
    c
}
/// SCSI READ CAPACITY(10) CDB.
pub fn scsi_read_capacity10() -> [u8; 10] { let mut c = [0u8; 10]; c[0] = 0x25; c }
/// SCSI TEST UNIT READY CDB.
pub fn scsi_test_unit_ready() -> [u8; 6] { [0u8; 6] }
/// SCSI INQUIRY CDB (`len` = allocation length).
pub fn scsi_inquiry(len: u8) -> [u8; 6] { let mut c = [0u8; 6]; c[0] = 0x12; c[4] = len; c }

// ── Detected MSC interface (from the config-descriptor walk in device.rs) ────

/// A bulk-only mass-storage interface: its bulk-IN + bulk-OUT endpoints.
#[derive(Clone, Copy)]
pub struct MscIface {
    pub iface:     u8,
    pub ep_in:     u8,  // bEndpointAddress (bit7=1)
    pub ep_out:    u8,  // bEndpointAddress (bit7=0)
    pub mps_in:    u16,
    pub mps_out:   u16,
    pub burst_in:  u8,  // SuperSpeed companion bMaxBurst (0 = none / non-SS)
    pub burst_out: u8,
}

// ── Running driver state (Copy: lives in the registry, copied out for I/O) ────

/// State for a configured BOT mass-storage device. `Copy` (all fields are Copy:
/// DmaRegion + scalars) so `MscBlock::read_blocks` can copy it OUT of the SLOTS
/// registry, run bulk transfers WITHOUT holding the SLOTS lock (avoiding the
/// `wait_for`→dispatch→`with_slot` re-lock deadlock), then write back the
/// advanced ring cursors — the same pattern hub `handle_port` uses for EP0.
#[derive(Clone, Copy)]
pub struct MscState {
    pub ctrl:        u8, // owning xHCI controller index
    pub slot_id:     u8,
    pub dci_in:      u8,
    pub dci_out:     u8,
    pub ring_in:     DmaRegion,
    pub enq_in:      usize,
    pub cyc_in:      bool,
    pub ring_out:    DmaRegion,
    pub enq_out:     usize,
    pub cyc_out:     bool,
    pub data:        DmaRegion, // scratch page: CBW@0, data@0, CSW@CSW_OFF
    pub tag:         u32,
    pub block_size:  u32,
    pub block_count: u64,
    pub max_lun:     u8,
}

// CSW lands at this page offset, clear of the largest data payload (a 2048-byte
// ISO sector read by SectorScale; READ CAPACITY = 8 B; INQUIRY = 36 B).
const CSW_OFF: usize = 3072;

/// Configure the device's two bulk endpoints (one xHCI Configure Endpoint
/// command), GET MAX LUN, then bring the LUN up (TEST UNIT READY + READ
/// CAPACITY). `dev` must already be in the Configured state (SET_CONFIGURATION
/// issued by `device::configure_msc`). Returns running state on success.
pub fn configure_endpoints(x: &mut Xhci, dev: &mut UsbDevice, mi: &MscIface) -> Option<MscState> {
    use ::xhci::context::{Input32Byte, Input64Byte, InputHandler, EndpointType};

    let dci_out = 2 * (mi.ep_out & 0x0F);       // OUT endpoint: even DCI
    let dci_in  = 2 * (mi.ep_in & 0x0F) + 1;    // IN endpoint:  odd DCI
    let max_dci = dci_in.max(dci_out);

    let ring_in  = crate::memory::dma::alloc(1)?;
    let ring_out = crate::memory::dma::alloc(1)?;
    let data     = crate::memory::dma::alloc(1)?;
    crate::usb::xhci::ring::init_link(ring_in.virt,  ring_in.phys.as_u64(),  true);
    crate::usb::xhci::ring::init_link(ring_out.virt, ring_out.phys.as_u64(), true);

    let csz = x.regs.capability.hccparams1.read_volatile().context_size();

    macro_rules! fill {
        ($input:expr) => {{
            {
                let ctrl = $input.control_mut();
                ctrl.set_add_context_flag(0);               // A0 = slot
                ctrl.set_add_context_flag(dci_out as usize);
                ctrl.set_add_context_flag(dci_in as usize);
            }
            {
                let dc = $input.device_mut();
                {
                    let slot = dc.slot_mut();
                    slot.set_context_entries(max_dci);
                    slot.set_root_hub_port_number(dev.port);
                    slot.set_speed(dev.speed);
                }
                {
                    let ep = dc.endpoint_mut(dci_out as usize);
                    ep.set_endpoint_type(EndpointType::BulkOut);
                    ep.set_max_packet_size(mi.mps_out);
                    ep.set_max_burst_size(mi.burst_out); // SuperSpeed companion bMaxBurst
                    ep.set_average_trb_length(mi.mps_out.max(8));
                    ep.set_tr_dequeue_pointer(ring_out.phys.as_u64());
                    ep.set_dequeue_cycle_state();
                    ep.set_error_count(3);
                }
                {
                    let ep = dc.endpoint_mut(dci_in as usize);
                    ep.set_endpoint_type(EndpointType::BulkIn);
                    ep.set_max_packet_size(mi.mps_in);
                    ep.set_max_burst_size(mi.burst_in); // SuperSpeed companion bMaxBurst
                    ep.set_average_trb_length(mi.mps_in.max(8));
                    ep.set_tr_dequeue_pointer(ring_in.phys.as_u64());
                    ep.set_dequeue_cycle_state();
                    ep.set_error_count(3);
                }
            }
            let bytes = core::mem::size_of_val(&$input);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    &$input as *const _ as *const u8,
                    dev.input_ctx.virt.as_mut_ptr::<u8>(),
                    bytes,
                );
            }
        }};
    }
    if csz {
        let mut input = Input64Byte::new_64byte();
        fill!(input);
    } else {
        let mut input = Input32Byte::new_32byte();
        fill!(input);
    }

    // Configure Endpoint command (type 12).
    let in_phys = dev.input_ctx.phys.as_u64();
    crate::usb::xhci::ring::enqueue_cmd(x, [
        (in_phys & 0xFFFF_FFF0) as u32,
        (in_phys >> 32) as u32,
        0u32,
        (dev.slot_id as u32) << 24,
    ], 12);
    let ev = crate::usb::xhci::ring::wait_cmd(x)?;
    let code = crate::usb::xhci::ring::completion_code(&ev);
    if code != 1 {
        crate::bwarn!("msc", "config_ep FAIL code={} slot={}", code, dev.slot_id);
        return None;
    }
    crate::binfo!("msc", "config_ep ok slot={} in_dci={} out_dci={}", dev.slot_id, dci_in, dci_out);

    // GET MAX LUN (class request 0xA1/0xFE). STALL → single LUN.
    let max_lun = {
        let buf = crate::memory::dma::alloc(1);
        let v = match buf {
            Some(b) => {
                let n = crate::usb::control::control_in(x, dev, crate::usb::control::Setup {
                    req_type: 0xA1, request: 0xFE, value: 0, index: mi.iface as u16, length: 1,
                }, &b);
                let r = match n {
                    Some(got) if got >= 1 => unsafe { core::ptr::read_volatile(b.virt.as_ptr::<u8>()) },
                    _ => 0,
                };
                crate::memory::dma::dealloc(b);
                r
            }
            None => 0,
        };
        v
    };

    let mut st = MscState {
        ctrl: x.idx,
        slot_id: dev.slot_id, dci_in, dci_out,
        ring_in, enq_in: 0, cyc_in: true,
        ring_out, enq_out: 0, cyc_out: true,
        data, tag: 1, block_size: 512, block_count: 0, max_lun,
    };

    // INQUIRY (best-effort; some devices want it before TUR).
    let _ = bot_xfer(x, &mut st, &scsi_inquiry(36), true, 36);

    // TEST UNIT READY — poll until ready (flash power-on lag), bounded ~2s.
    let mut ready = false;
    let deadline = crate::boot::clock::elapsed_ms() + 2000;
    while crate::boot::clock::elapsed_ms() < deadline {
        match bot_xfer(x, &mut st, &scsi_test_unit_ready(), false, 0) {
            Some(0) => { ready = true; break; }
            _ => { let t = crate::boot::clock::elapsed_ms() + 100; while crate::boot::clock::elapsed_ms() < t { core::hint::spin_loop(); } }
        }
    }
    if !ready { crate::bwarn!("msc", "unit not ready slot={}", dev.slot_id); return None; }

    // READ CAPACITY(10): last-LBA (BE u32) @0, block-size (BE u32) @4.
    match bot_xfer(x, &mut st, &scsi_read_capacity10(), true, 8) {
        Some(0) => {
            let p = st.data.virt.as_ptr::<u8>();
            let rd = |o: usize| unsafe { core::ptr::read_volatile(p.add(o)) };
            let last_lba = u32::from_be_bytes([rd(0), rd(1), rd(2), rd(3)]);
            let bsize    = u32::from_be_bytes([rd(4), rd(5), rd(6), rd(7)]);
            st.block_size  = if bsize == 0 { 512 } else { bsize };
            st.block_count = (last_lba as u64) + 1;
        }
        other => { crate::bwarn!("msc", "read capacity failed status={:?}", other); return None; }
    }

    crate::binfo!("msc", "MSC ready slot={} bsize={} blocks={} max_lun={}",
        st.slot_id, st.block_size, st.block_count, st.max_lun);
    Some(st)
}

/// Run one BOT command: CBW(OUT) → optional data phase → CSW(IN). Data (if any)
/// is left at `st.data` offset 0 for the caller to read. Returns the CSW status
/// byte (0 = good), or `None` on any USB-level failure.
pub fn bot_xfer(x: &mut Xhci, st: &mut MscState, cdb: &[u8], data_in: bool, data_len: u32) -> Option<u8> {
    let tag = st.tag;
    st.tag = st.tag.wrapping_add(1);
    let dphys = st.data.phys.as_u64();

    // CBW → bulk-OUT.
    let cbw = build_cbw(tag, data_len, data_in, 0, cdb);
    unsafe {
        core::ptr::copy_nonoverlapping(cbw.as_ptr(), st.data.virt.as_mut_ptr::<u8>(), CBW_LEN);
    }
    let (c, _) = crate::usb::xhci::bulk::bulk_xfer(
        x, st.slot_id, st.dci_out, &st.ring_out, &mut st.enq_out, &mut st.cyc_out,
        dphys, CBW_LEN as u32)?;
    if c != 1 { crate::bwarn!("msc", "CBW out code={}", c); return None; }

    // Data phase (into/out of page offset 0).
    if data_len > 0 {
        let (dci, ring, enq, cyc) = if data_in {
            (st.dci_in, &st.ring_in, &mut st.enq_in, &mut st.cyc_in)
        } else {
            (st.dci_out, &st.ring_out, &mut st.enq_out, &mut st.cyc_out)
        };
        let (c, _) = crate::usb::xhci::bulk::bulk_xfer(x, st.slot_id, dci, ring, enq, cyc, dphys, data_len)?;
        if c != 1 && c != 13 { crate::bwarn!("msc", "data phase code={}", c); return None; }
    }

    // CSW → bulk-IN at CSW_OFF (clear of the data payload).
    let (c, _) = crate::usb::xhci::bulk::bulk_xfer(
        x, st.slot_id, st.dci_in, &st.ring_in, &mut st.enq_in, &mut st.cyc_in,
        dphys + CSW_OFF as u64, CSW_LEN as u32)?;
    if c != 1 { crate::bwarn!("msc", "CSW in code={}", c); return None; }

    let mut csw = [0u8; CSW_LEN];
    unsafe {
        core::ptr::copy_nonoverlapping(
            st.data.virt.as_ptr::<u8>().add(CSW_OFF), csw.as_mut_ptr(), CSW_LEN);
    }
    parse_csw(&csw, tag).map(|c| c.status)
}

/// Read `n` sectors at `lba` into `out` (len == n * block_size). Data is staged
/// through `st.data` page 0, so `n * block_size` must be ≤ CSW_OFF.
fn bot_read(x: &mut Xhci, st: &mut MscState, lba: u32, n: u16, out: &mut [u8]) -> Result<(), BlockError> {
    let len = n as u32 * st.block_size;
    match bot_xfer(x, st, &scsi_read10(lba, n), true, len) {
        Some(0) => {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    st.data.virt.as_ptr::<u8>(), out.as_mut_ptr(), len as usize);
            }
            Ok(())
        }
        // Loud on failure: the boot screen (no serial on real HW) shows whether
        // the read failed at the USB level (None) or with a SCSI CSW status != 0.
        other => {
            crate::bwarn!("msc", "READ(10) lba={} n={} len={} -> csw={:?}", lba, n, len, other);
            Err(BlockError::Io)
        }
    }
}

// ── BlockDevice front-end ────────────────────────────────────────────────────

/// A `BlockDevice` view of an enumerated MSC slot. Holds only the slot id +
/// cached geometry; each I/O copies the live `MscState` out of the registry,
/// runs transfers, writes it back.
pub struct MscBlock { ctrl: u8, slot: u8, block_size: u32, block_count: u64 }

/// Build an `MscBlock` for the first enumerated MSC slot, if any.
pub fn first_block() -> Option<MscBlock> {
    let (ctrl, slot) = crate::usb::registry::first_msc_slot()?;
    let st = crate::usb::registry::msc_state(ctrl, slot)?;
    Some(MscBlock { ctrl, slot, block_size: st.block_size, block_count: st.block_count })
}

impl BlockDevice for MscBlock {
    fn block_size(&self) -> u32 { self.block_size }
    fn block_count(&self) -> u64 { self.block_count }
    fn write_blocks(&mut self, _lba: u64, _buf: &[u8]) -> Result<(), BlockError> { Err(BlockError::Io) }

    fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let bs = self.block_size as u64;
        if bs == 0 || buf.len() as u64 % bs != 0 { return Err(BlockError::BadAlignment); }
        let cell = crate::usb::CTRLS.get().ok_or(BlockError::Io)?;
        let mut g = cell.lock();
        let x = g.get_mut(self.ctrl as usize).ok_or(BlockError::Io)?;
        // Copy state out (releases SLOTS before any bulk transfer).
        let mut st = crate::usb::registry::msc_state(self.ctrl, self.slot).ok_or(BlockError::Io)?;
        let max_per = (CSW_OFF as u64 / bs).max(1); // sectors that fit page-0
        let total = buf.len() as u64 / bs;
        let mut done = 0u64;
        let mut result = Ok(());
        while done < total {
            let n = core::cmp::min(max_per, total - done) as u16;
            let lo = (done * bs) as usize;
            let hi = lo + (n as u64 * bs) as usize;
            if let Err(e) = bot_read(x, &mut st, (lba + done) as u32, n, &mut buf[lo..hi]) {
                result = Err(e);
                break;
            }
            done += n as u64;
        }
        crate::usb::registry::set_msc_state(self.ctrl, self.slot, st);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*; extern crate std;
    #[test] fn cbw_read10() {
        let cdb = scsi_read10(5, 2);
        let cbw = build_cbw(0xDEAD_BEEF, 2 * 512, true, 0, &cdb);
        assert_eq!(&cbw[0..4], b"USBC");
        assert_eq!(u32::from_le_bytes([cbw[4], cbw[5], cbw[6], cbw[7]]), 0xDEAD_BEEF);
        assert_eq!(u32::from_le_bytes([cbw[8], cbw[9], cbw[10], cbw[11]]), 2 * 512);
        assert_eq!(cbw[12], 0x80);
        assert_eq!(cbw[14], 10);
        assert_eq!(cbw[15], 0x28);
        assert_eq!(u32::from_be_bytes([cbw[17], cbw[18], cbw[19], cbw[20]]), 5);
    }
    #[test] fn csw_ok() {
        let mut c = [0u8; 13];
        c[0..4].copy_from_slice(b"USBS");
        c[4..8].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        let p = parse_csw(&c, 0xDEAD_BEEF).unwrap();
        assert_eq!(p.status, 0);
        assert_eq!(p.residue, 0);
    }
    #[test] fn csw_bad_sig() { assert!(parse_csw(&[0u8; 13], 0).is_none()); }
    #[test] fn csw_tag_mismatch() {
        let mut c = [0u8; 13];
        c[0..4].copy_from_slice(b"USBS");
        c[4..8].copy_from_slice(&1u32.to_le_bytes());
        assert!(parse_csw(&c, 2).is_none());
    }
}
