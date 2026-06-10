//! Realtek RTL8188EU USB-WiFi driver — SP1: bind + register transport.
//!
//! Recognises the dongle `0bda:8179` during USB enumeration, sets up register
//! I/O over the EP0 control pipe using the rtl8xxxu vendor protocol, and reads
//! `REG_SYS_CFG` to prove the transport works. Firmware download, RF/BB/MAC
//! init, scan, association and the data path live in later subprojects (SP2-5);
//! see docs/superpowers/specs/2026-06-08-rtl8188eu-wifi-resources.md.
//!
//! Reference: Linux `rtl8xxxu` / `r8188eu`. The vendor request constants and
//! register offsets below come from that reference — verify on real hardware.

use core::ptr::read_volatile;

// 802.11 protocol layer (probe-request build + beacon parse) for the scan (SP3).
// Not yet driven over the radio — that needs the RTL TX/RX descriptor path +
// channel/RF init (SP3b/SP4). Allowed dead until then.
#[allow(dead_code)]
pub mod ieee80211;

// RTL8188EU init register tables (MAC/PHY_REG/AGC/RADIO_A), ported verbatim from
// rtl8xxxu 8188e.c. Applied by init_mac/init_phy_bb/init_phy_rf in SP3c-3.
pub mod tables;

// WPA2-PSK 4-way-handshake supplicant crypto (SP-WIFI-3). Pure offline math,
// consumed by the association/key path in SP-WIFI-2/4. Allowed dead until then;
// boot-checks runs its known-answer self-test (`wpa2::selftest`).
#[allow(dead_code)]
pub mod wpa2;

// EAPOL-Key framing for the WPA2 4-way handshake (SP-WIFI-4).
#[allow(dead_code)]
pub mod eapol;

use crate::usb::control::{control_in, control_out_data, Setup};
use crate::usb::device::UsbDevice;
use crate::usb::xhci::Xhci;
use crate::memory::dma::DmaRegion;

/// RTL8188EU firmware blob, uploaded into the chip's 8051 RAM during bring-up
/// (SP2). 15262 bytes, 32-byte header (signature 0x88e1) + body.
static FW: &[u8] = include_bytes!("fw/rtl8188eufw.bin");

/// USB vendor identifiers for the supported dongle.
pub const WIFI_VID: u16 = 0x0BDA;
pub const WIFI_PID: u16 = 0x8179;

/// rtl8xxxu register-access vendor request: bRequest=0x05, wIndex=0; a read uses
/// bmRequestType 0xC0 (Dev→Host, Vendor, Device), a write 0x40, wValue = offset.
const VENDOR_REQ: u8 = 0x05;
const REQ_TYPE_READ: u8 = 0xC0;
const REQ_TYPE_WRITE: u8 = 0x40;

// ── Firmware-download registers/bits (RTL8188E, from rtl8xxxu reference) ──────
const REG_SYS_FUNC_EN:    u16 = 0x0002; // SYS_FUNC: CPU enable @ BIT10, USBA @ BIT2
const SYS_FUNC_CPU_EN:    u16 = 0x0400; // BIT10 = SYS_FUNC_CPU_ENABLE (8051 CPU clock).
                                        // NOT 0x04 (BIT2 = SYS_FUNC_USBA = USB analog
                                        // PHY enable): clearing BIT2 kills the USB link
                                        // mid-transfer → code-4 + pipe wedge (old bug).
const REG_MCUFWDL:        u16 = 0x0080; // +2 (0x0082) low 3 bits = page select
const MCUFWDL_EN:         u8  = 0x01;
const MCUFWDL_RAM_DL_SEL: u8  = 0x80;
const WINTINI_RDY:        u8  = 0x40;   // fw init done (WINT_INIT_READY), MCUFWDL
const MCUFWDL_CSUM_RPT:   u8  = 0x04;   // checksum report / download done
const MCUFWDL_DL_RDY:     u8  = 0x02;   // MCU_FW_DL_READY
const MCUFWDL_RST_8051:   u32 = 1 << 19; // 8051 reset bit in the 32-bit MCUFWDL
const REG_FW_START_ADDR:  u16 = 0x1000; // page FIFO; page selected via MCUFWDL+2
const REG_RSV_CTRL:       u16 = 0x001C; // write-protect lock for SYS_FUNC_EN etc.
const FW_HDR_SIZE:        usize = 32;   // 8188e fw header (sig 0x88e1) skipped
const FW_SIG:             u16 = 0x88E1;
const FW_PAGE:            usize = 4096;

// ── Power-on sequence registers/bits (rtl8188eu_power_on, rtl8xxxu reference) ─
const REG_APS_FSMCO:     u16 = 0x0004;
const REG_AFE_XTAL_CTRL: u16 = 0x0024;
const REG_LPLDO_CTRL:    u16 = 0x0023;
const REG_CR:            u16 = 0x0100;
const APS_FSMCO_HW_POWERDOWN: u32 = 1 << 15;
const APS_FSMCO_HW_SUSPEND:   u32 = 1 << 11;
const APS_FSMCO_PCIE:         u32 = 1 << 12;
const APS_FSMCO_MAC_ENABLE:   u32 = 1 << 8;
const APS_FSMCO_PWR_RDY:      u32 = 1 << 17; // emu_to_active power-ready poll bit
const SYS_FUNC_BBRSTB:        u8  = 1 << 0;
const SYS_FUNC_BB_GLB_RSTN:   u8  = 1 << 1;
const AFE_XTAL_GATE:          u32 = 1 << 23;
const LPLDO_BIT4:             u8  = 1 << 4;
// REG_CR enable mask: HCI TX/RX DMA, TX/RX DMA, protocol, schedule, security,
// caltimer = BIT0..5 | BIT9 | BIT10.
const CR_ENABLE: u16 = 0x063F;

/// System config register — readable right after reset; non-zero/non-0xFFFFFFFF
/// proves the control pipe + vendor protocol reach the chip.
const REG_SYS_CFG: u16 = 0x00F0;

/// Endpoint/iface layout discovered at enumeration. The bulk endpoints are saved
/// for SP2 (firmware download / TX-RX); SP1 does not use them.
pub struct WifiIface {
    pub iface:   u8,
    pub ep_in:   u8,
    pub ep_out:  u8,
    pub mps_in:  u16,
    pub mps_out: u16,
    /// Number of bulk-OUT endpoints exposed — selects the TX queue layout
    /// (rtl8xxxu config_endpoints_no_sie). We drive mgmt frames on the first.
    pub nr_out_eps: u8,
}

/// Per-slot state kept in the USB registry. `Copy` (DmaRegion is Copy, like
/// `MscState`). Holds the configured bulk IN/OUT transfer rings + cursors so
/// `send_frame`/`recv_frame` can drive `bulk_xfer` after enumeration.
#[derive(Clone, Copy)]
pub struct WifiState {
    pub ctrl:    u8,
    pub slot_id: u8,
    pub iface:   u8,
    pub dci_in:  u8,
    pub dci_out: u8,
    pub ring_in:  DmaRegion,
    pub enq_in:   usize,
    pub cyc_in:   bool,
    pub ring_out: DmaRegion,
    pub enq_out:  usize,
    pub cyc_out:  bool,
    pub data:    DmaRegion, // scratch page: TxDesc+frame on TX, RxDesc+frame on RX
    pub mps_in:  u16,
    pub mps_out: u16,
    /// Lazy bring-up latch: false until the first `wifiscan` runs power-on +
    /// firmware + MAC/BB/RF init. Keeps enumeration fast (no chip bring-up at boot).
    pub radio_up: bool,
    /// 802.11 transmit sequence number (12-bit), incremented per frame.
    pub tx_seq: u16,
    /// Our station MAC, read from EFUSE at bring-up (SA for TX frames).
    pub mac: [u8; 6],
    /// Bulk-OUT endpoint count (TX queue layout). See `WifiIface::nr_out_eps`.
    pub nr_out_eps: u8,
    /// Next H2C mailbox index (0..H2C_MAX_MBOX), round-robin per command.
    pub h2c_mbox: u8,
}

/// Read `len` (1/2/4) register bytes via a vendor control-IN. Returns the value
/// zero-extended into a u32 (little-endian), or None on a short/failed transfer.
fn reg_read(x: &mut Xhci, dev: &mut UsbDevice, addr: u16, len: u16) -> Option<u32> {
    let buf = crate::memory::dma::alloc(1)?;
    let r = (|| {
        let s = Setup { req_type: REQ_TYPE_READ, request: VENDOR_REQ, value: addr, index: 0, length: len };
        if control_in(x, dev, s, &buf)? < len { return None; }
        let rd = |o: usize| unsafe { read_volatile(buf.virt.as_ptr::<u8>().add(o)) };
        let mut v = 0u32;
        for i in 0..len as usize {
            v |= (rd(i) as u32) << (8 * i);
        }
        Some(v)
    })();
    crate::memory::dma::dealloc(buf);
    r
}

#[inline] pub fn reg_read8(x: &mut Xhci, dev: &mut UsbDevice, addr: u16) -> Option<u8> {
    reg_read(x, dev, addr, 1).map(|v| v as u8)
}
#[inline] pub fn reg_read16(x: &mut Xhci, dev: &mut UsbDevice, addr: u16) -> Option<u16> {
    reg_read(x, dev, addr, 2).map(|v| v as u16)
}
#[inline] pub fn reg_read32(x: &mut Xhci, dev: &mut UsbDevice, addr: u16) -> Option<u32> {
    reg_read(x, dev, addr, 4)
}

/// Write `len` (1/2/4) register bytes via a vendor control-OUT (data stage).
#[allow(dead_code)]
fn reg_write(x: &mut Xhci, dev: &mut UsbDevice, addr: u16, val: u32, len: u16) -> bool {
    let buf = match crate::memory::dma::alloc(1) { Some(b) => b, None => return false };
    unsafe {
        let p = buf.virt.as_ptr::<u8>() as *mut u8;
        for i in 0..len as usize { core::ptr::write_volatile(p.add(i), (val >> (8 * i)) as u8); }
    }
    let s = Setup { req_type: REQ_TYPE_WRITE, request: VENDOR_REQ, value: addr, index: 0, length: len };
    let ok = control_out_data(x, dev, s, &buf);
    crate::memory::dma::dealloc(buf);
    if !ok { crate::bwarn!("wifi", "reg_write FAIL addr=0x{:04x} len={}", addr, len); }
    ok
}

#[allow(dead_code)] #[inline]
fn reg_write8(x: &mut Xhci, dev: &mut UsbDevice, addr: u16, v: u8) -> bool { reg_write(x, dev, addr, v as u32, 1) }
#[allow(dead_code)] #[inline]
fn reg_write16(x: &mut Xhci, dev: &mut UsbDevice, addr: u16, v: u16) -> bool { reg_write(x, dev, addr, v as u32, 2) }
#[allow(dead_code)] #[inline]
fn reg_write32(x: &mut Xhci, dev: &mut UsbDevice, addr: u16, v: u32) -> bool { reg_write(x, dev, addr, v, 4) }

/// Read-modify-write a 1-byte register: set/clear `mask` bits.
#[allow(dead_code)]
fn reg_set8(x: &mut Xhci, dev: &mut UsbDevice, addr: u16, mask: u8, set: bool) {
    if let Some(v) = reg_read8(x, dev, addr) {
        let nv = if set { v | mask } else { v & !mask };
        reg_write8(x, dev, addr, nv);
    }
}

/// Upload the firmware blob into the chip's 8051 RAM (RTL8188E sequence, from
/// the rtl8xxxu reference). NOT yet wired into enumeration — the on-chip power
/// rails / clock must be brought up first (SP2b: power-on table), otherwise the
/// register accesses time out. Exposed so SP2b can call it after power_on.
///
/// ⚠️ UNVERIFIED on hardware — SP1 transport (vendor reg I/O) has not yet been
/// confirmed on a real dongle.
#[allow(dead_code)]
pub fn download_firmware(x: &mut Xhci, dev: &mut UsbDevice) -> bool {
    if FW.len() < FW_HDR_SIZE { crate::bwarn!("wifi", "fw blob too small"); return false; }
    let sig = (FW[0] as u16) | ((FW[1] as u16) << 8);
    let body = if sig == FW_SIG { &FW[FW_HDR_SIZE..] } else { &FW[..] };
    crate::binfo!("wifi", "fw {} bytes, body {} (sig {:#06x})", FW.len(), body.len(), sig);

    // Enter download mode (rtl8xxxu_download_firmware order):
    // 1. Enable the 8051 CPU — 16-bit SYS_FUNC, BIT10 (SYS_FUNC_CPU_ENABLE).
    if let Some(v) = reg_read16(x, dev, REG_SYS_FUNC_EN) {
        reg_write16(x, dev, REG_SYS_FUNC_EN, v | SYS_FUNC_CPU_EN);
    }
    // 2. If firmware already resident (RAM-select), clear it.
    if let Some(v) = reg_read8(x, dev, REG_MCUFWDL) {
        if v & MCUFWDL_RAM_DL_SEL != 0 { reg_write8(x, dev, REG_MCUFWDL, 0); }
    }
    // 3. Enable MCU firmware download.
    if let Some(v) = reg_read8(x, dev, REG_MCUFWDL) { reg_write8(x, dev, REG_MCUFWDL, v | MCUFWDL_EN); }
    // 4. Hold the 8051 in reset for the download — clear BIT19 of the 32-bit MCUFWDL.
    if let Some(v) = reg_read32(x, dev, REG_MCUFWDL) { reg_write32(x, dev, REG_MCUFWDL, v & !MCUFWDL_RST_8051); }
    // 5. Arm the checksum report.
    if let Some(v) = reg_read8(x, dev, REG_MCUFWDL) { reg_write8(x, dev, REG_MCUFWDL, v | MCUFWDL_CSUM_RPT); }
    crate::binfo!("wifi", "fw dl: MCUFWDL=0x{:02x}", reg_read8(x, dev, REG_MCUFWDL).unwrap_or(0xFF));

    // Upload page-by-page (4 KiB, page selected in MCUFWDL+2[2:0]). Within a page
    // the firmware is written in small blocks to REG_FW_START_ADDR + offset: a
    // single 4 KiB control-OUT fails with a USB Transaction Error (EP0 mps=64),
    // verified on hardware — so cap each control transfer at FW_BLOCK bytes.
    const FW_BLOCK: usize = 64;
    let buf = match crate::memory::dma::alloc(1) { Some(b) => b, None => return false };
    let pages = body.len().div_ceil(FW_PAGE);
    let mut ok = true;
    'pages: for p in 0..pages {
        if let Some(v) = reg_read8(x, dev, REG_MCUFWDL + 2) {
            reg_write8(x, dev, REG_MCUFWDL + 2, (v & 0xF8) | (p as u8 & 0x07));
        }
        let page_off = p * FW_PAGE;
        let page_len = FW_PAGE.min(body.len() - page_off);
        let mut o = 0;
        while o < page_len {
            let csz = (page_len - o).min(FW_BLOCK);
            unsafe {
                let dst = buf.virt.as_ptr::<u8>() as *mut u8;
                for i in 0..csz { core::ptr::write_volatile(dst.add(i), body[page_off + o + i]); }
            }
            let s = Setup {
                req_type: REQ_TYPE_WRITE, request: VENDOR_REQ,
                value: REG_FW_START_ADDR + o as u16, index: 0, length: csz as u16,
            };
            if !control_out_data(x, dev, s, &buf) {
                crate::bwarn!("wifi", "fw page {} off {} write failed", p, o);
                ok = false;
                break 'pages;
            }
            o += csz;
        }
    }
    crate::memory::dma::dealloc(buf);
    crate::binfo!("wifi", "fw upload pages={} ok={}", pages, ok);
    if !ok { return false; }

    // 8. Leave download mode — 16-bit clear of MCU_FW_DL_ENABLE.
    if let Some(v) = reg_read16(x, dev, REG_MCUFWDL) {
        reg_write16(x, dev, REG_MCUFWDL, v & !(MCUFWDL_EN as u16));
    }

    // start_firmware: wait checksum, mark ready, wait init-ready.
    // Polls are bounded LOW: a write to a wedged device makes every following
    // control transfer time out (~200 ms each); a 1000-iteration poll would hang
    // the whole boot for minutes. Keep counts small so boot always proceeds.
    let mut csum = false;
    for _ in 0..50 {
        if let Some(v) = reg_read32(x, dev, REG_MCUFWDL) {
            if (v as u8) & MCUFWDL_CSUM_RPT != 0 { csum = true; break; }
        }
    }
    crate::binfo!("wifi", "fw dl: csum_report={}", csum);
    // Mark MCU_FW_DL_READY, clear WINT_INIT_READY (32-bit RMW on MCUFWDL).
    if let Some(v) = reg_read32(x, dev, REG_MCUFWDL) {
        let nv = (v | MCUFWDL_DL_RDY as u32) & !(WINTINI_RDY as u32);
        reg_write32(x, dev, REG_MCUFWDL, nv);
    }
    // reset_8051 (rtl8188eu_reset_8051): toggle SYS_FUNC_CPU_ENABLE (BIT10) off→on
    // to restart the 8051 so it runs the loaded firmware. The earlier wedge was a
    // bug — we cleared BIT2 (SYS_FUNC_USBA = the USB analog PHY), which dropped the
    // USB link. BIT10 is the CPU clock and is safe to toggle; the USB PHY stays on.
    if let Some(v) = reg_read16(x, dev, REG_SYS_FUNC_EN) {
        let off_ok = reg_write16(x, dev, REG_SYS_FUNC_EN, v & !SYS_FUNC_CPU_EN);
        let on_ok  = reg_write16(x, dev, REG_SYS_FUNC_EN, v | SYS_FUNC_CPU_EN);
        crate::binfo!("wifi", "reset_8051 sysfunc={:#06x} off_ok={} on_ok={}", v, off_ok, on_ok);
    }
    // Poll WINT_INIT_READY (BIT6 of MCUFWDL) on the same pipe — bounded.
    let mut ready = false;
    for _ in 0..50 {
        if let Some(v) = reg_read32(x, dev, REG_MCUFWDL) {
            if (v as u8) & WINTINI_RDY != 0 { ready = true; break; }
        }
    }
    if ready {
        crate::binfo!("wifi", "fw ready");
    } else {
        crate::bwarn!("wifi", "fw verified (csum={}) but WINT_INIT_READY not set", csum);
    }
    csum || ready
}

/// Chip power-on sequence (RTL8188EU `rtl8188eu_power_on`, rtl8xxxu reference):
/// emu→active power rails + AFE/LDO + enable the MAC DMA/queue engines. Must run
/// before `download_firmware`. ⚠️ UNVERIFIED on hardware.
#[allow(dead_code)]
pub fn power_on(x: &mut Xhci, dev: &mut UsbDevice) -> bool {
    // disabled_to_emu: drop the HW power-down bit.
    if let Some(v) = reg_read16(x, dev, REG_APS_FSMCO) {
        reg_write16(x, dev, REG_APS_FSMCO, (v as u32 & !APS_FSMCO_HW_POWERDOWN) as u16);
    }
    // emu_to_active: wait for the power-ready bit.
    let mut rdy = false;
    for _ in 0..100 {
        if let Some(v) = reg_read32(x, dev, REG_APS_FSMCO) {
            if v & APS_FSMCO_PWR_RDY != 0 { rdy = true; break; }
        }
    }
    if !rdy { crate::bwarn!("wifi", "power_on: PWR_RDY timeout (continuing)"); }
    // Release BB resets.
    if let Some(v) = reg_read8(x, dev, REG_SYS_FUNC_EN) {
        reg_write8(x, dev, REG_SYS_FUNC_EN, v & !(SYS_FUNC_BBRSTB | SYS_FUNC_BB_GLB_RSTN));
    }
    // Gate the AFE crystal.
    if let Some(v) = reg_read32(x, dev, REG_AFE_XTAL_CTRL) {
        reg_write32(x, dev, REG_AFE_XTAL_CTRL, v | AFE_XTAL_GATE);
    }
    // Clear power-down + suspend/PCIe.
    if let Some(v) = reg_read16(x, dev, REG_APS_FSMCO) {
        reg_write16(x, dev, REG_APS_FSMCO, (v as u32 & !APS_FSMCO_HW_POWERDOWN) as u16);
    }
    if let Some(v) = reg_read16(x, dev, REG_APS_FSMCO) {
        reg_write16(x, dev, REG_APS_FSMCO, (v as u32 & !(APS_FSMCO_HW_SUSPEND | APS_FSMCO_PCIE)) as u16);
    }
    // Enable the MAC, then wait for the enable bit to self-clear.
    if let Some(v) = reg_read32(x, dev, REG_APS_FSMCO) {
        reg_write32(x, dev, REG_APS_FSMCO, v | APS_FSMCO_MAC_ENABLE);
    }
    for _ in 0..100 {
        if let Some(v) = reg_read32(x, dev, REG_APS_FSMCO) {
            if v & APS_FSMCO_MAC_ENABLE == 0 { break; }
        }
    }
    // LPLDO: clear bit 4.
    if let Some(v) = reg_read8(x, dev, REG_LPLDO_CTRL) {
        reg_write8(x, dev, REG_LPLDO_CTRL, v & !LPLDO_BIT4);
    }
    // Enable HCI/MAC DMA + queue engines.
    reg_write16(x, dev, REG_CR, CR_ENABLE);
    crate::binfo!("wifi", "power_on done (pwr_rdy={})", rdy);
    true
}

// ── TX-engine bring-up (SP-WIFI-1) ───────────────────────────────────────────
// The chip TX path needs its internal packet-buffer pages allocated before it
// will accept a bulk-OUT frame. rtl8xxxu does this across init_device in two
// places: RQPN + queue-priority + RX FIFO boundary *before* firmware download,
// and the TX buffer boundary + LLT + quirks *after* the PHY/RF tables. We mirror
// that split so the (working) firmware download and RX path are untouched.

/// 8188eu uses `config_endpoints_no_sie`: the bulk-OUT endpoint count picks which
/// DMA queues exist. Returns (ep_tx_count, hi_en, lo_en, norm_en).
fn ep_queue_config(nr_out_eps: u8) -> (u8, bool, bool, bool) {
    match nr_out_eps {
        0 | 1 => (1, true, false, false),
        2     => (2, true, false, true),
        _     => (3, true, true, true), // 3..=6
    }
}

/// Per-traffic-class queue selectors (hiq, mgq, bkq, beq, viq, voq) for the
/// REG_TRXDMA_CTRL mapping — rtl8xxxu_init_queue_priority cases 1/2/3.
fn queue_select(ep_tx_count: u8, hi: bool, lo: bool, nq: bool) -> (u16, u16, u16, u16, u16, u16) {
    match ep_tx_count {
        1 => {
            let h = if hi { TRXDMA_QUEUE_HIGH } else if lo { TRXDMA_QUEUE_LOW } else { TRXDMA_QUEUE_NORMAL };
            (h, h, h, h, h, h)
        }
        2 => {
            let (h, l) = if hi && lo { (TRXDMA_QUEUE_HIGH, TRXDMA_QUEUE_LOW) }
                else if nq && lo     { (TRXDMA_QUEUE_NORMAL, TRXDMA_QUEUE_LOW) }
                else                 { (TRXDMA_QUEUE_HIGH, TRXDMA_QUEUE_NORMAL) }; // hi && nq
            (h, h, l, l, h, h) // hiq=mgq=viq=voq=h, bkq=beq=l
        }
        _ => (TRXDMA_QUEUE_HIGH, TRXDMA_QUEUE_HIGH, TRXDMA_QUEUE_LOW,
              TRXDMA_QUEUE_LOW, TRXDMA_QUEUE_NORMAL, TRXDMA_QUEUE_HIGH),
    }
}

/// Reserve packet-buffer pages per queue (rtl8xxxu_init_queue_reserved_page).
fn init_queue_reserved_page(x: &mut Xhci, dev: &mut UsbDevice, hi: bool, lo: bool, nq: bool) {
    let hq = if hi { TX_PAGE_NUM_HI_PQ_8188E } else { 0 };
    let lq = if lo { TX_PAGE_NUM_LO_PQ_8188E } else { 0 };
    let nq_pages = if nq { TX_PAGE_NUM_NORM_PQ_8188E } else { 0 };
    let eq = 0u32;
    reg_write32(x, dev, REG_RQPN_NPQ, (nq_pages << RQPN_NPQ_SHIFT) | (eq << RQPN_EPQ_SHIFT));
    let pubq = TX_TOTAL_PAGE_NUM_8188E - hq - lq - nq_pages - 1;
    let val = RQPN_LOAD
        | (hq << RQPN_HI_PQ_SHIFT)
        | (lq << RQPN_LO_PQ_SHIFT)
        | (pubq << RQPN_PUB_PQ_SHIFT);
    reg_write32(x, dev, REG_RQPN, val);
}

/// Map the traffic-class queues onto the DMA queues (REG_TRXDMA_CTRL).
fn init_queue_priority(x: &mut Xhci, dev: &mut UsbDevice, ep_tx_count: u8, hi: bool, lo: bool, nq: bool) {
    let (hiq, mgq, bkq, beq, viq, voq) = queue_select(ep_tx_count, hi, lo, nq);
    let cur = reg_read16(x, dev, REG_TRXDMA_CTRL).unwrap_or(0) & 0x7;
    let val = cur
        | (voq << TRXDMA_CTRL_VOQ_SHIFT)
        | (viq << TRXDMA_CTRL_VIQ_SHIFT)
        | (beq << TRXDMA_CTRL_BEQ_SHIFT)
        | (bkq << TRXDMA_CTRL_BKQ_SHIFT)
        | (mgq << TRXDMA_CTRL_MGQ_SHIFT)
        | (hiq << TRXDMA_CTRL_HIQ_SHIFT);
    reg_write16(x, dev, REG_TRXDMA_CTRL, val);
}

/// Write one LLT entry (rtl8xxxu_llt_write): REG_LLT_INIT = OP_WRITE|addr<<8|data,
/// then poll until the op goes inactive. Returns false on timeout.
fn llt_write(x: &mut Xhci, dev: &mut UsbDevice, addr: u8, data: u8) -> bool {
    reg_write32(x, dev, REG_LLT_INIT, LLT_OP_WRITE | ((addr as u32) << 8) | data as u32);
    for _ in 0..20 {
        if let Some(v) = reg_read32(x, dev, REG_LLT_INIT) {
            if (v & LLT_OP_MASK) == LLT_OP_INACTIVE { return true; }
        }
    }
    false
}

/// Build the packet-buffer link list (rtl8xxxu_init_llt_table): pages
/// 0..total form the TX chain, total points to 0xff (end), the rest form a
/// ring buffer that wraps to total+1.
fn init_llt_table(x: &mut Xhci, dev: &mut UsbDevice) -> bool {
    let last_tx_page = TX_TOTAL_PAGE_NUM_8188E as u8;
    let last_entry = LAST_LLT_ENTRY_8188E;
    for i in 0..last_tx_page {
        if !llt_write(x, dev, i, i + 1) { return false; }
    }
    if !llt_write(x, dev, last_tx_page, 0xff) { return false; }
    for i in (last_tx_page + 1)..last_entry {
        if !llt_write(x, dev, i, i + 1) { return false; }
    }
    llt_write(x, dev, last_entry, last_tx_page + 1)
}

/// TX-engine init that must run *before* the firmware download (rtl8xxxu order):
/// reserve queue pages, map queue priorities, set the RX FIFO boundary.
fn tx_engine_pre_fw(x: &mut Xhci, dev: &mut UsbDevice, nr_out_eps: u8) {
    let (ep_tx_count, hi, lo, nq) = ep_queue_config(nr_out_eps);
    init_queue_reserved_page(x, dev, hi, lo, nq);
    init_queue_priority(x, dev, ep_tx_count, hi, lo, nq);
    reg_write16(x, dev, REG_TRXFF_BNDY + 2, TRXFF_BOUNDARY_8188E);
    crate::binfo!("wifi", "tx-engine pre-fw: out_eps={} ep_tx_count={}", nr_out_eps, ep_tx_count);
}

/// TX-engine init that must run *after* the PHY/RF tables (rtl8xxxu order):
/// TX buffer page boundaries, PBP page size, the LLT, the gen2 TX-DMA quirk,
/// TX report timer, and the firmware/HW TX-queue control bits.
fn tx_engine_post_rf(x: &mut Xhci, dev: &mut UsbDevice) {
    // TX buffer boundary = last TX page + 1.
    let bdny = (TX_TOTAL_PAGE_NUM_8188E + 1) as u8;
    reg_write8(x, dev, REG_TXPKTBUF_BCNQ_BDNY, bdny);
    reg_write8(x, dev, REG_TXPKTBUF_MGQ_BDNY, bdny);
    reg_write8(x, dev, REG_TXPKTBUF_WMAC_LBK_BF_HD, bdny);
    reg_write8(x, dev, REG_TRXFF_BNDY, bdny);
    reg_write8(x, dev, REG_TDECTRL + 1, bdny);
    // PBP page size (128B rx/tx).
    reg_write8(x, dev, REG_PBP,
        (PBP_PAGE_SIZE_128 << PBP_PAGE_SIZE_TX_SHIFT) | (PBP_PAGE_SIZE_128 << PBP_PAGE_SIZE_RX_SHIFT));
    let llt = init_llt_table(x, dev);
    // gen2 USB quirk: drop-data-enable on the TX DMA offset check.
    if let Some(v) = reg_read32(x, dev, REG_TXDMA_OFFSET_CHK) {
        reg_write32(x, dev, REG_TXDMA_OFFSET_CHK, v | TXDMA_OFFSET_DROP_DATA_EN);
    }
    // TX report (8188e needs BIT0; timer for sw rate control).
    if let Some(v) = reg_read8(x, dev, REG_TX_REPORT_CTRL) {
        reg_write8(x, dev, REG_TX_REPORT_CTRL, v | 0x01 | TX_REPORT_CTRL_TIMER_ENABLE);
    }
    reg_write8(x, dev, REG_TX_REPORT_CTRL + 1, 0x02);
    reg_write16(x, dev, REG_TX_REPORT_TIME, 0xcdf0);
    if let Some(v) = reg_read8(x, dev, 0xa3) { reg_write8(x, dev, 0xa3, v & 0xf8); }
    // FW/HW TX-queue control: AMPDU retry + ack for transmitted mgmt frames.
    if let Some(v) = reg_read32(x, dev, REG_FWHW_TXQ_CTRL) {
        reg_write32(x, dev, REG_FWHW_TXQ_CTRL, v | FWHW_TXQ_CTRL_AMPDU_RETRY | FWHW_TXQ_CTRL_XMIT_MGMT_ACK);
    }
    crate::binfo!("wifi", "tx-engine post-rf: boundary+pbp+llt({})+report done", llt);
}

/// Read one EFUSE byte (rtl8xxxu_read_efuse8): program the address, trigger a
/// read (clear bit7 of CTRL+3), poll BIT31 of CTRL for data-valid.
fn read_efuse8(x: &mut Xhci, dev: &mut UsbDevice, offset: u16) -> Option<u8> {
    reg_write8(x, dev, REG_EFUSE_CTRL + 1, (offset & 0xff) as u8);
    let v = reg_read8(x, dev, REG_EFUSE_CTRL + 2)?;
    reg_write8(x, dev, REG_EFUSE_CTRL + 2, (v & 0xfc) | (((offset >> 8) & 0x03) as u8));
    let v = reg_read8(x, dev, REG_EFUSE_CTRL + 3)?;
    reg_write8(x, dev, REG_EFUSE_CTRL + 3, v & 0x7f);
    let mut ok = false;
    for _ in 0..500 {
        if reg_read32(x, dev, REG_EFUSE_CTRL)? & (1 << 31) != 0 { ok = true; break; }
    }
    if !ok { return None; }
    crate::boot::clock::udelay(50);
    Some((reg_read32(x, dev, REG_EFUSE_CTRL)? & 0xff) as u8)
}

/// Decode the EFUSE into a 512-byte map and return the station MAC (8188eu MAC
/// at map offset 0xd7, gated on rtl_id 0x8129). Mirrors rtl8xxxu_read_efuse +
/// rtl8188eu_parse_efuse. Returns None on a transport/format error.
fn read_efuse_mac(x: &mut Xhci, dev: &mut UsbDevice) -> Option<[u8; 6]> {
    // Loader clock / 1.2V power / reset gates the EFUSE controller needs.
    reg_write8(x, dev, REG_EFUSE_ACCESS, EFUSE_ACCESS_ENABLE);
    if let Some(v) = reg_read16(x, dev, REG_9346CR) { let _ = v; }
    if let Some(v) = reg_read16(x, dev, REG_SYS_ISO_CTRL) {
        if v & SYS_ISO_PWC_EV12V == 0 { reg_write16(x, dev, REG_SYS_ISO_CTRL, v | SYS_ISO_PWC_EV12V); }
    }
    if let Some(v) = reg_read16(x, dev, REG_SYS_FUNC_EN) {
        if v & SYS_FUNC_ELDR == 0 { reg_write16(x, dev, REG_SYS_FUNC_EN, v | SYS_FUNC_ELDR); }
    }
    if let Some(v) = reg_read16(x, dev, REG_SYS_CLKR) {
        if v & SYS_CLK_LOADER_ENABLE == 0 || v & SYS_CLK_ANA8M == 0 {
            reg_write16(x, dev, REG_SYS_CLKR, v | SYS_CLK_LOADER_ENABLE | SYS_CLK_ANA8M);
        }
    }

    let mut map = [0xffu8; EFUSE_MAP_LEN];
    let mut addr: u16 = 0;
    while (addr as usize) < EFUSE_MAP_LEN {
        let header = read_efuse8(x, dev, addr)?;
        addr += 1;
        if header == 0xff { break; }
        let (offset, word_mask) = if header & 0x1f == 0x0f {
            let ext = read_efuse8(x, dev, addr)?;
            addr += 1;
            if ext & 0x0f == 0x0f { continue; } // all words disabled
            (((header as usize & 0xe0) >> 5) | ((ext as usize & 0xf0) >> 1), ext & 0x0f)
        } else {
            (((header as usize) >> 4) & 0x0f, header & 0x0f)
        };
        let mut map_addr = offset * 8;
        for i in 0..EFUSE_MAX_WORD_UNIT {
            if word_mask & (1 << i) != 0 { map_addr += 2; continue; }
            let lo = read_efuse8(x, dev, addr)?; addr += 1;
            if map_addr >= EFUSE_MAP_LEN - 1 { break; }
            map[map_addr] = lo; map_addr += 1;
            let hi = read_efuse8(x, dev, addr)?; addr += 1;
            map[map_addr] = hi; map_addr += 1;
        }
    }
    reg_write8(x, dev, REG_EFUSE_ACCESS, EFUSE_ACCESS_DISABLE);

    let rtl_id = (map[0] as u16) | ((map[1] as u16) << 8);
    if rtl_id != EFUSE_8188EU_RTL_ID {
        crate::bwarn!("wifi", "efuse rtl_id=0x{:04x} (want 0x{:04x})", rtl_id, EFUSE_8188EU_RTL_ID);
        return None;
    }
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&map[EFUSE_8188EU_MAC_OFFSET..EFUSE_8188EU_MAC_OFFSET + 6]);
    Some(mac)
}

/// SP2 bring-up: power-on, TX-engine pre-firmware init, then upload firmware.
/// ⚠️ UNVERIFIED on hardware — the register transport (SP1) has not yet been
/// confirmed on a real dongle.
pub fn bring_up(x: &mut Xhci, dev: &mut UsbDevice, nr_out_eps: u8) -> bool {
    if !power_on(x, dev) { return false; }
    tx_engine_pre_fw(x, dev, nr_out_eps);
    download_firmware(x, dev)
}

// ── SP3c-1: RF register access (FPGA0 LSSI/HSSI 3-wire, path A) ───────────────
// RF registers are NOT in MAC space — they're reached through a baseband serial
// bridge. Offsets/encoding from Linux rtl8xxxu regs.h + rtl8xxxu_read/write_rfreg.
const REG_FPGA0_XA_HSSI_PARM1:  u16 = 0x0820;
const REG_FPGA0_XA_HSSI_PARM2:  u16 = 0x0824;
const REG_FPGA0_XA_LSSI_PARM:   u16 = 0x0840;
const REG_FPGA0_XA_LSSI_READBK: u16 = 0x08A0;
const REG_HSPI_XA_READBACK:     u16 = 0x08B8;
const LSSI_DATA_MASK:        u32 = 0x000F_FFFF; // 20-bit RF data
const LSSI_ADDR_SHIFT:       u32 = 20;
const HSSI_PARM1_PI:         u32 = 1 << 8;
const HSSI_PARM2_ADDR_MASK:  u32 = 0x7F80_0000;
const HSSI_PARM2_ADDR_SHIFT: u32 = 23;
const HSSI_PARM2_EDGE_READ:  u32 = 1 << 31;

// ── SP3c-3: MAC/BB/RF init registers (rtl8xxxu regs.h) ───────────────────────
const REG_RF_CTRL:              u16 = 0x001F;
const REG_MAX_AGGR_NUM:         u16 = 0x04CA;
const REG_FPGA0_XA_RF_INT_OE:   u16 = 0x0860;
const REG_FPGA0_XA_RF_SW_CTRL:  u16 = 0x0870;
const SYSF_BBRSTB:      u16 = 1 << 0;
const SYSF_BB_GLB_RSTN: u16 = 1 << 1;
const SYSF_USBA:        u16 = 1 << 2;
const SYSF_USBD:        u16 = 1 << 4;
const SYSF_DIO_RF:      u16 = 1 << 13;
const RF_CTRL_VAL:      u8  = 0x07; // RF_ENABLE(BIT0)|RF_RSTB(BIT1)|RF_SDMRSTB(BIT2)
const FPGA0_RF_RFENV:   u16 = 1 << 4;
const HSSI_3WIRE_ADDR_LEN: u32 = 0x400;
const HSSI_3WIRE_DATA_LEN: u32 = 0x800;

// ── SP3c-4: RX enable (usb_quirks CR MAC-RX, RCR/MAR, modems, enable_rf) ──────
const REG_TRXFF_BNDY:            u16 = 0x0114;
const REG_EARLY_MODE_CTRL_8188E: u16 = 0x04D0;
const REG_RX_DRVINFO_SZ:         u16 = 0x060F;
const REG_RCR:                   u16 = 0x0608;
const REG_MAR:                   u16 = 0x0620;
const REG_FPGA0_RF_MODE:         u16 = 0x0800;
const REG_OFDM0_TRX_PATH_EN:     u16 = 0x0C04;
const REG_TXPAUSE:               u16 = 0x0522;
const CR_MAC_TX_ENABLE: u16 = 1 << 6;
const CR_MAC_RX_ENABLE: u16 = 1 << 7;
const FPGA_RF_MODE_CCK:  u32 = 1 << 24;
const FPGA_RF_MODE_OFDM: u32 = 1 << 25;
const OFDM_RF_PATH_RX_MASK: u32 = 0x0F;
const OFDM_RF_PATH_TX_MASK: u32 = 0xF0;
const OFDM_RF_PATH_RX_A: u32 = 1 << 0;
const OFDM_RF_PATH_TX_A: u32 = 1 << 4;
// RCR = ACCEPT_PHYS_MATCH|MCAST|BCAST|MGMT_FRAME|HTC_LOC_CTRL|PHYSTAT|ICV|MIC,
// plus ACCEPT_DATA_FRAME (BIT11). The HW test showed rx total=75 data=0 during
// the 4-way window: with only APM, directed data frames (the AP's EAPOL msg1)
// never reach the bulk-IN, so the handshake stalls. BIT11 accepts all data so
// msg1/msg3 arrive. (The earlier "instability" was the smoltcp DNS panic + a
// bad USB stick, not a data-frame flood — that attribution was wrong.)
const RCR_VAL: u32 = 0x7000_680E;

// ── SP3c-6: channel set (config_channel, 20 MHz) ─────────────────────────────
const REG_BW_OPMODE:    u16 = 0x0603;
const REG_FPGA1_RF_MODE: u16 = 0x0900;
const BW_OPMODE_20MHZ:  u8  = 1 << 2;
const FPGA_RF_MODE_BW:  u32 = 1 << 0; // FPGA_RF_MODE bandwidth bit
const RF6052_REG_MODE_AG: u8 = 0x18;  // RF channel + BW switch (the rf[0x18] reg)
const MODE_AG_CHANNEL_MASK: u32 = 0x3FF;
const MODE_AG_BW_MASK:      u32 = (1 << 10) | (1 << 11);
const MODE_AG_BW_20MHZ:     u32 = (1 << 10) | (1 << 11);

// ── TX engine init (RQPN + queue priority + LLT + boundary), rtl8xxxu ref ─────
// Without these the on-chip TX packet buffer has no pages → the MAC NAKs every
// bulk-OUT forever → the host transfer times out (the `tx code=0` we saw). The
// 8188eu page-allocation constants come straight from rtl8188eu_fops.
const TX_TOTAL_PAGE_NUM_8188E:   u32 = 0xa9;
const TX_PAGE_NUM_HI_PQ_8188E:   u32 = 0x29;
const TX_PAGE_NUM_LO_PQ_8188E:   u32 = 0x1c;
const TX_PAGE_NUM_NORM_PQ_8188E: u32 = 0x1c;
const LAST_LLT_ENTRY_8188E:      u8  = 175;
const TRXFF_BOUNDARY_8188E:      u16 = 0x25ff;

const REG_RQPN:          u16 = 0x0200;
const RQPN_HI_PQ_SHIFT:  u32 = 0;
const RQPN_LO_PQ_SHIFT:  u32 = 8;
const RQPN_PUB_PQ_SHIFT: u32 = 16;
const RQPN_LOAD:         u32 = 1 << 31;
const REG_RQPN_NPQ:      u16 = 0x0214;
const RQPN_NPQ_SHIFT:    u32 = 0;
const RQPN_EPQ_SHIFT:    u32 = 16;

const REG_TRXDMA_CTRL:       u16 = 0x010c;
const TRXDMA_CTRL_VOQ_SHIFT: u16 = 4;
const TRXDMA_CTRL_VIQ_SHIFT: u16 = 6;
const TRXDMA_CTRL_BEQ_SHIFT: u16 = 8;
const TRXDMA_CTRL_BKQ_SHIFT: u16 = 10;
const TRXDMA_CTRL_MGQ_SHIFT: u16 = 12;
const TRXDMA_CTRL_HIQ_SHIFT: u16 = 14;
const TRXDMA_QUEUE_LOW:    u16 = 1;
const TRXDMA_QUEUE_NORMAL: u16 = 2;
const TRXDMA_QUEUE_HIGH:   u16 = 3;

const REG_LLT_INIT:    u16 = 0x01e0;
const LLT_OP_INACTIVE: u32 = 0x0;
const LLT_OP_WRITE:    u32 = 0x1 << 30;
const LLT_OP_MASK:     u32 = 0x3 << 30;

const REG_TXPKTBUF_BCNQ_BDNY:     u16 = 0x0424;
const REG_TXPKTBUF_MGQ_BDNY:      u16 = 0x0425;
const REG_TXPKTBUF_WMAC_LBK_BF_HD: u16 = 0x045d;
const REG_TDECTRL: u16 = 0x0208;
const REG_PBP:     u16 = 0x0104;
const PBP_PAGE_SIZE_128:     u8 = 0x1;
const PBP_PAGE_SIZE_RX_SHIFT: u8 = 0;
const PBP_PAGE_SIZE_TX_SHIFT: u8 = 4;

const REG_TXDMA_OFFSET_CHK:    u16 = 0x020c;
const TXDMA_OFFSET_DROP_DATA_EN: u32 = 1 << 9;
const REG_TX_REPORT_CTRL:        u16 = 0x04ec;
const TX_REPORT_CTRL_TIMER_ENABLE: u8 = 1 << 1;
const REG_TX_REPORT_TIME:        u16 = 0x04f0;
const REG_FWHW_TXQ_CTRL:         u16 = 0x0420;
const FWHW_TXQ_CTRL_AMPDU_RETRY:   u32 = 1 << 7;
const FWHW_TXQ_CTRL_XMIT_MGMT_ACK: u32 = 1 << 12;

// ── EFUSE read (station MAC), rtl8xxxu rtl8xxxu_read_efuse / 8188eu parse ─────
const REG_EFUSE_CTRL:       u16 = 0x0030; // [+1]=addr lo, [+2][1:0]=addr hi, [+3] bit7=read, [0] data+BIT31 valid
const REG_EFUSE_ACCESS:     u16 = 0x00cf;
const EFUSE_ACCESS_ENABLE:  u8  = 0x69;
const EFUSE_ACCESS_DISABLE: u8  = 0x00;
const REG_9346CR:           u16 = 0x000a;
const REG_SYS_ISO_CTRL:     u16 = 0x0000;
const SYS_ISO_PWC_EV12V:    u16 = 1 << 15;
const SYS_FUNC_ELDR:        u16 = 1 << 12; // REG_SYS_FUNC_EN bit (loader reset)
const REG_SYS_CLKR:         u16 = 0x0008;
const SYS_CLK_ANA8M:        u16 = 1 << 1;
const SYS_CLK_LOADER_ENABLE: u16 = 1 << 5;
const EFUSE_MAP_LEN:        usize = 512;
const EFUSE_MAX_WORD_UNIT:  usize = 4;
const EFUSE_8188EU_MAC_OFFSET: usize = 0xd7;
const EFUSE_8188EU_RTL_ID:  u16 = 0x8129;

// ── SP-WIFI-2: STA opmode + BSSID + AID + H2C media-connect (rtl8xxxu ref) ────
const REG_MSR:              u16 = 0x0102; // media-status: link type, port0 bits[1:0]
const MSR_LINKTYPE_STATION: u8  = 0x2;
const REG_BSSID:            u16 = 0x0618; // 6 bytes
const REG_BCN_MAX_ERR:      u16 = 0x055d;
const REG_BCN_PSR_RPT:      u16 = 0x06a8; // joinbss: 0xc000 | AID
const REG_HMBOX_0:          u16 = 0x01d0; // H2C mailbox 0 (4 bytes), +4 per box
const REG_HMTFR:            u16 = 0x01cc; // H2C mailbox busy flags (bit per box)
const H2C_MAX_MBOX:         u8  = 4;
const H2C_MEDIA_STATUS_RPT: u8  = 0x01;   // gen2 H2C command
const H2C_MACID_ROLE_AP:    u8  = 2;      // from the STA, macid 0 = the AP

// ── SP-WIFI-4: HW CCMP key CAM + security config (rtl8xxxu reference) ─────────
const REG_CAM_CMD:        u16 = 0x0670;
const CAM_CMD_POLLING:    u32 = 1 << 31;
const CAM_CMD_WRITE:      u32 = 1 << 16;
const CAM_CMD_KEY_SHIFT:  u32 = 3;        // 8 dwords per CAM key entry
const REG_CAM_WRITE:      u16 = 0x0674;
const CAM_WRITE_VALID:    u32 = 1 << 15;
const REG_SECURITY_CFG:   u16 = 0x0680;
// TX/RX sec enable + default-key (uni + broadcast). rtl8xxxu set_key value 0xCF.
const SEC_CFG_VAL:        u8  = 0xCF;
const CR_SECURITY_ENABLE: u16 = 1 << 9;
const CCMP_CIPHER_NIBBLE: u32 = 0x04;     // WLAN_CIPHER_SUITE_CCMP & 0x0f
// TX queue selector for EAPOL data frames (best-effort), vs QSEL_MGMT.
const QSEL_BE:            u32 = 0x00;

/// Write a 20-bit RF register (path A) via the LSSI interface (rtl8xxxu
/// write_rfreg). Used by the RADIO_A init table + config_channel (SP3c-3/6).
#[allow(dead_code)]
fn write_rfreg(x: &mut Xhci, dev: &mut UsbDevice, reg: u8, val: u32) {
    let word = ((reg as u32) << LSSI_ADDR_SHIFT) | (val & LSSI_DATA_MASK);
    reg_write32(x, dev, REG_FPGA0_XA_LSSI_PARM, word);
    crate::boot::clock::udelay(1);
}

/// Read a 20-bit RF register (path A) via the BB readback path (rtl8xxxu
/// read_rfreg). The value is only meaningful after BB init (SP3c-3); pre-init it
/// merely proves the FPGA0 register path is live.
#[allow(dead_code)]
fn read_rfreg(x: &mut Xhci, dev: &mut UsbDevice, reg: u8) -> u32 {
    let hssia = reg_read32(x, dev, REG_FPGA0_XA_HSSI_PARM2).unwrap_or(0);
    let mut sel = hssia & !HSSI_PARM2_ADDR_MASK;
    sel |= ((reg as u32) << HSSI_PARM2_ADDR_SHIFT) & HSSI_PARM2_ADDR_MASK;
    sel |= HSSI_PARM2_EDGE_READ;
    reg_write32(x, dev, REG_FPGA0_XA_HSSI_PARM2, hssia & !HSSI_PARM2_EDGE_READ);
    crate::boot::clock::udelay(10);
    reg_write32(x, dev, REG_FPGA0_XA_HSSI_PARM2, sel);
    crate::boot::clock::udelay(100);
    reg_write32(x, dev, REG_FPGA0_XA_HSSI_PARM2, hssia | HSSI_PARM2_EDGE_READ);
    crate::boot::clock::udelay(10);
    let pi = reg_read32(x, dev, REG_FPGA0_XA_HSSI_PARM1).unwrap_or(0);
    let rb = if pi & HSSI_PARM1_PI != 0 {
        reg_read32(x, dev, REG_HSPI_XA_READBACK)
    } else {
        reg_read32(x, dev, REG_FPGA0_XA_LSSI_READBK)
    };
    rb.unwrap_or(0) & LSSI_DATA_MASK
}

// ── Register-table apply scaffolding (data filled in SP3c-2; used by SP3c-3) ──
/// MAC init table: {reg16, val8}; stop at the (0xFFFF, 0xFF) terminator.
#[allow(dead_code)]
fn apply_reg8_table(x: &mut Xhci, dev: &mut UsbDevice, t: &[(u16, u8)]) {
    for &(a, v) in t {
        if a == 0xFFFF && v == 0xFF { break; }
        reg_write8(x, dev, a, v);
    }
}
/// BB/AGC init table: {reg16, val32}; stop at (0xFFFF, 0xFFFFFFFF).
#[allow(dead_code)]
fn apply_reg32_table(x: &mut Xhci, dev: &mut UsbDevice, t: &[(u16, u32)]) {
    for &(a, v) in t {
        if a == 0xFFFF && v == 0xFFFF_FFFF { break; }
        reg_write32(x, dev, a, v);
        crate::boot::clock::udelay(1);
    }
}
/// RADIO table: {rfreg8, val32}; regs 0xFE..0xF9 are delay markers (rtl8xxxu
/// rf-table opcode convention), not writes; stop at (0xFF, 0xFFFFFFFF).
#[allow(dead_code)]
fn apply_rf_table(x: &mut Xhci, dev: &mut UsbDevice, t: &[(u8, u32)]) {
    for &(reg, v) in t {
        match reg {
            0xFF if v == 0xFFFF_FFFF => break,
            0xFE => crate::boot::clock::mdelay(50),
            0xFD => crate::boot::clock::mdelay(5),
            0xFC => crate::boot::clock::mdelay(1),
            0xFB => crate::boot::clock::udelay(50),
            0xFA => crate::boot::clock::udelay(5),
            0xF9 => crate::boot::clock::udelay(1),
            _ => { write_rfreg(x, dev, reg, v); crate::boot::clock::udelay(1); }
        }
    }
}

/// SP3c-1 self-test: exercise the RF/FPGA0 register path + confirm it doesn't
/// wedge EP0. Reads two RF regs (path A) and logs them; pre-BB-init the values
/// may be 0/garbage — the point is the path runs and the device stays alive.
#[allow(dead_code)]
fn rf_selftest(x: &mut Xhci, dev: &mut UsbDevice) {
    let r00 = read_rfreg(x, dev, 0x00);
    let r18 = read_rfreg(x, dev, 0x18);
    crate::binfo!("wifi", "rf selftest rf[0x00]=0x{:05x} rf[0x18]=0x{:05x} tsc_per_ms={}",
        r00, r18, crate::boot::clock::tsc_per_ms());
}

// ── SP3c-3: MAC / BB / RF init (rtl8xxxu init_mac / rtl8188eu_init_phy_bb /
//    rtl8xxxu_init_phy_rf), applied after `fw ready` to bring the radio alive. ──

/// MAC init: apply the MAC reg table + 8188E max-aggregation default.
fn init_mac(x: &mut Xhci, dev: &mut UsbDevice) {
    apply_reg8_table(x, dev, tables::MAC_INIT);
    reg_write16(x, dev, REG_MAX_AGGR_NUM, 0x0707);
}

/// Baseband init (rtl8188eu_init_phy_bb): BB/RF reset release, then the PHY_REG
/// and AGC tables.
fn init_phy_bb(x: &mut Xhci, dev: &mut UsbDevice) {
    if let Some(v) = reg_read16(x, dev, REG_SYS_FUNC_EN) {
        reg_write16(x, dev, REG_SYS_FUNC_EN, v | SYSF_BB_GLB_RSTN | SYSF_BBRSTB | SYSF_DIO_RF);
    }
    reg_write8(x, dev, REG_RF_CTRL, RF_CTRL_VAL);
    reg_write8(x, dev, REG_SYS_FUNC_EN, (SYSF_USBA | SYSF_USBD | SYSF_BB_GLB_RSTN | SYSF_BBRSTB) as u8);
    apply_reg32_table(x, dev, tables::PHY_INIT);
    apply_reg32_table(x, dev, tables::AGC_TAB);
}

/// RF path-A init (rtl8xxxu_init_phy_rf, RF_A): RF_INT_OE / 3-wire-len preamble,
/// the RADIO_A table, then restore RFENV.
fn init_phy_rf(x: &mut Xhci, dev: &mut UsbDevice) {
    let rfsi = reg_read16(x, dev, REG_FPGA0_XA_RF_SW_CTRL).unwrap_or(0) & FPGA0_RF_RFENV;
    if let Some(v) = reg_read32(x, dev, REG_FPGA0_XA_RF_INT_OE) {
        reg_write32(x, dev, REG_FPGA0_XA_RF_INT_OE, v | (1 << 20));
    }
    crate::boot::clock::udelay(1);
    if let Some(v) = reg_read32(x, dev, REG_FPGA0_XA_RF_INT_OE) {
        reg_write32(x, dev, REG_FPGA0_XA_RF_INT_OE, v | (1 << 4));
    }
    crate::boot::clock::udelay(1);
    if let Some(v) = reg_read32(x, dev, REG_FPGA0_XA_HSSI_PARM2) {
        reg_write32(x, dev, REG_FPGA0_XA_HSSI_PARM2, v & !HSSI_3WIRE_ADDR_LEN);
    }
    crate::boot::clock::udelay(1);
    if let Some(v) = reg_read32(x, dev, REG_FPGA0_XA_HSSI_PARM2) {
        reg_write32(x, dev, REG_FPGA0_XA_HSSI_PARM2, v & !HSSI_3WIRE_DATA_LEN);
    }
    crate::boot::clock::udelay(1);
    apply_rf_table(x, dev, tables::RADIOA_INIT);
    if let Some(v) = reg_read16(x, dev, REG_FPGA0_XA_RF_SW_CTRL) {
        reg_write16(x, dev, REG_FPGA0_XA_RF_SW_CTRL, (v & !FPGA0_RF_RFENV) | rfsi);
    }
}

/// SP3c-3: full radio bring-up after `fw ready`. Applies MAC → BB(+AGC) → RF
/// tables, then the RF self-test (readback should now be live, not 0x00000).
/// NOTE: synchronous + ~0.5-1s (≈420 register writes + RADIO_A msleep markers).
/// TEMP: invoked at enumeration for SP3c-3 validation; moves behind a lazy
/// `wifiscan` command in SP3c-6/7 so normal boots stay fast.
pub fn bring_up_radio(x: &mut Xhci, dev: &mut UsbDevice) {
    crate::binfo!("wifi", "radio init: mac ({} rows)", tables::MAC_INIT.len());
    init_mac(x, dev);
    crate::binfo!("wifi", "radio init: bb (phy {} + agc {})", tables::PHY_INIT.len(), tables::AGC_TAB.len());
    init_phy_bb(x, dev);
    crate::binfo!("wifi", "radio init: rf (radioa {})", tables::RADIOA_INIT.len());
    init_phy_rf(x, dev);
    tx_engine_post_rf(x, dev);
    rx_enable(x, dev);
    crate::binfo!("wifi", "radio init done");
}

/// SP3c-4: enable the receive path. RX FIFO boundary (must precede the MAC-RX
/// enable on 8188E), MAC TX/RX enable (usb_quirks), RX driver-info size, RX
/// config (accept mgmt+bcast+mcast), accept-all multicast, the CCK+OFDM modems,
/// and enable_rf (rtl8188e_enable_rf). After this the MAC can DMA received
/// frames to the bulk-IN pipe. Channel tuning is config_channel (SP3c-6).
fn rx_enable(x: &mut Xhci, dev: &mut UsbDevice) {
    // RX page boundary (8188E latches it wrong if set after MAC-RX enable).
    reg_write16(x, dev, REG_TRXFF_BNDY + 2, 0x25FF);
    // usb_quirks: enable MAC TX + RX (after TRXFF_BNDY).
    if let Some(v) = reg_read16(x, dev, REG_CR) {
        reg_write16(x, dev, REG_CR, v | CR_MAC_TX_ENABLE | CR_MAC_RX_ENABLE);
    }
    reg_write8(x, dev, REG_EARLY_MODE_CTRL_8188E + 3, 0x01);
    // RX driver-info size (8-byte units) — the chip prepends PHY status.
    reg_write8(x, dev, REG_RX_DRVINFO_SZ, 4);
    // Receive config + accept-all multicast.
    reg_write32(x, dev, REG_RCR, RCR_VAL);
    reg_write32(x, dev, REG_MAR, 0xFFFF_FFFF);
    reg_write32(x, dev, REG_MAR + 4, 0xFFFF_FFFF);
    // Enable the CCK + OFDM baseband modems.
    if let Some(v) = reg_read32(x, dev, REG_FPGA0_RF_MODE) {
        reg_write32(x, dev, REG_FPGA0_RF_MODE, v | FPGA_RF_MODE_CCK | FPGA_RF_MODE_OFDM);
    }
    // enable_rf (rtl8188e_enable_rf): RF on, OFDM path RX_A|TX_A, unpause TX.
    reg_write8(x, dev, REG_RF_CTRL, RF_CTRL_VAL);
    if let Some(v) = reg_read32(x, dev, REG_OFDM0_TRX_PATH_EN) {
        let nv = (v & !(OFDM_RF_PATH_RX_MASK | OFDM_RF_PATH_TX_MASK))
            | OFDM_RF_PATH_RX_A | OFDM_RF_PATH_TX_A;
        reg_write32(x, dev, REG_OFDM0_TRX_PATH_EN, nv);
    }
    reg_write8(x, dev, REG_TXPAUSE, 0x00);
    crate::binfo!("wifi", "rx enabled (CR MAC-RX, RCR={:#010x}, modems+RF on)", RCR_VAL);
}

/// Probe an enumerated device: match `0bda:8179`, find its bulk endpoints, set
/// the configuration, and read the chip's system-config register. Returns the
/// endpoint layout on success, or None if this isn't our dongle / setup failed.
pub fn configure_wifi(x: &mut Xhci, dev: &mut UsbDevice) -> Option<WifiIface> {
    let buf = crate::memory::dma::alloc(1)?;
    let result = (|| {
        let rd = |o: usize| unsafe { read_volatile(buf.virt.as_ptr::<u8>().add(o)) };
        let rd16 = |o: usize| (rd(o) as u16) | ((rd(o + 1) as u16) << 8);

        // ── 1. Device descriptor → VID/PID gate ──────────────────────────────
        let sd = Setup { req_type: 0x80, request: 6, value: 0x0100, index: 0, length: 18 };
        if control_in(x, dev, sd, &buf)? < 18 { return None; }
        let vid = rd16(8);
        let pid = rd16(10);
        if vid != WIFI_VID || pid != WIFI_PID { return None; }

        // ── 2. Config header → wTotalLength + bConfigurationValue ─────────────
        let s9 = Setup { req_type: 0x80, request: 6, value: 0x0200, index: 0, length: 9 };
        if control_in(x, dev, s9, &buf)? < 9 { return None; }
        let total = (rd16(2)).min(4096);
        let cfg_val = rd(5);

        // ── 3. Full config block → first bulk IN/OUT of the first interface ───
        let s_all = Setup { req_type: 0x80, request: 6, value: 0x0200, index: 0, length: total };
        let n = control_in(x, dev, s_all, &buf)?;
        let n = (n.min(total)) as usize;

        let mut pos = 0usize;
        let mut iface_num = 0u8;
        let mut in_iface = false;
        let mut ep_in: Option<(u8, u16)> = None;
        let mut ep_out: Option<(u8, u16)> = None;
        let mut nr_out_eps = 0u8;
        while pos + 2 <= n {
            let blen = rd(pos) as usize;
            if blen == 0 || pos + blen > n { break; }
            match rd(pos + 1) {
                4 if pos + 9 <= n => {
                    // Interface descriptor — RTL8188EU has a single vendor-class
                    // interface; bind its endpoints.
                    iface_num = rd(pos + 2);
                    in_iface = true;
                    ep_in = None;
                    ep_out = None;
                    nr_out_eps = 0;
                }
                5 if in_iface && pos + 7 <= n => {
                    let addr = rd(pos + 2);
                    let attr = rd(pos + 3);
                    let mps = rd16(pos + 4);
                    if attr & 0x03 == 2 { // bulk
                        if addr & 0x80 != 0 { ep_in.get_or_insert((addr, mps)); }
                        else { ep_out.get_or_insert((addr, mps)); nr_out_eps += 1; }
                    }
                }
                _ => {}
            }
            pos += blen;
        }

        let (ep_in, mps_in) = ep_in?;
        let (ep_out, mps_out) = ep_out?;

        // ── 4. SET_CONFIGURATION ──────────────────────────────────────────────
        let ok = crate::usb::control::control_out(x, dev, Setup {
            req_type: 0x00, request: 9, value: cfg_val as u16, index: 0, length: 0,
        });
        if !ok { crate::bwarn!("wifi", "set_config failed"); return None; }

        // ── 5. Transport proof: read REG_SYS_CFG ──────────────────────────────
        match reg_read32(x, dev, REG_SYS_CFG) {
            Some(v) => crate::binfo!("wifi", "rtl8188eu sys_cfg=0x{:08x} (transport ok)", v),
            None    => crate::bwarn!("wifi", "sys_cfg read failed (verify vendor req on HW)"),
        }
        crate::binfo!("wifi", "rtl8188eu iface={} ep_in=0x{:02x} ep_out=0x{:02x} mps_in={} mps_out={} out_eps={}",
            iface_num, ep_in, ep_out, mps_in, mps_out, nr_out_eps);

        // SP2: attempt chip bring-up (power-on + firmware upload). Failure does
        // NOT abort the bind — the slot is still registered so later subprojects
        // can retry. UNVERIFIED on hardware.
        // Chip bring-up (power-on + firmware + MAC/BB/RF + scan) is LAZY — it runs
        // on the first `wifiscan` (run_scan), not here, so boot stays fast.
        Some(WifiIface { iface: iface_num, ep_in, ep_out, mps_in, mps_out, nr_out_eps })
    })();
    crate::memory::dma::dealloc(buf);
    result
}

// ── SP3b: bulk endpoint configuration + frame I/O ────────────────────────────

/// RTL8188EU TX descriptor size (txdesc40) prepended to every outgoing 802.11
/// frame on the bulk-OUT pipe. RX descriptor (rxdesc24) precedes each received
/// frame on bulk-IN. From the rtl8xxxu reference.
const TX_DESC_SIZE: usize = 32; // rtl8xxxu_txdesc32 (8188eu); was wrongly 40
const RX_DESC_SIZE: usize = 24;
const REG_MACID: u16 = 0x0610;  // station MAC (6 bytes), loaded from EEPROM
/// Management TX queue select (QSEL in txdw1 bits 8:12).
const QSEL_MGMT: u32 = 0x12;

/// Configure the two bulk endpoints (one xHCI Configure Endpoint command),
/// mirroring `msc::configure_endpoints`. Returns the running `WifiState` with
/// transfer rings. `dev` must already be Configured (configure_wifi set it).
pub fn configure_endpoints(x: &mut Xhci, dev: &mut UsbDevice, wi: &WifiIface) -> Option<WifiState> {
    use ::xhci::context::{Input32Byte, Input64Byte, InputHandler, EndpointType};

    let dci_out = 2 * (wi.ep_out & 0x0F);
    let dci_in  = 2 * (wi.ep_in & 0x0F) + 1;
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
                ctrl.set_add_context_flag(0);
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
                    ep.set_max_packet_size(wi.mps_out);
                    ep.set_average_trb_length(wi.mps_out.max(8));
                    ep.set_tr_dequeue_pointer(ring_out.phys.as_u64());
                    ep.set_dequeue_cycle_state();
                    ep.set_error_count(3);
                }
                {
                    let ep = dc.endpoint_mut(dci_in as usize);
                    ep.set_endpoint_type(EndpointType::BulkIn);
                    ep.set_max_packet_size(wi.mps_in);
                    ep.set_average_trb_length(wi.mps_in.max(8));
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
        crate::bwarn!("wifi", "config_ep FAIL code={} slot={}", code, dev.slot_id);
        crate::memory::dma::dealloc(ring_in);
        crate::memory::dma::dealloc(ring_out);
        crate::memory::dma::dealloc(data);
        return None;
    }
    crate::binfo!("wifi", "config_ep ok slot={} in_dci={} out_dci={}", dev.slot_id, dci_in, dci_out);

    Some(WifiState {
        ctrl: x.idx, slot_id: dev.slot_id, iface: wi.iface,
        dci_in, dci_out,
        ring_in, enq_in: 0, cyc_in: true,
        ring_out, enq_out: 0, cyc_out: true,
        data, mps_in: wi.mps_in, mps_out: wi.mps_out,
        radio_up: false, tx_seq: 0, mac: [0; 6], nr_out_eps: wi.nr_out_eps,
        h2c_mbox: 0,
    })
}

/// 16-bit XOR checksum over the first 32 bytes (csum field zeroed during compute).
/// rtl8xxxu_calc_tx_desc_csum — a wrong/missing csum hangs TX.
fn tx_desc_csum(d: &mut [u8; TX_DESC_SIZE]) {
    d[28] = 0; d[29] = 0;
    let mut c: u16 = 0;
    for i in 0..16 { c ^= u16::from_le_bytes([d[2 * i], d[2 * i + 1]]); }
    d[28..30].copy_from_slice(&c.to_le_bytes());
}

/// Build the 32-byte `rtl8xxxu_txdesc32` for `frame_len` payload bytes on TX
/// queue `qsel` (QSEL_MGMT for management, QSEL_BE for EAPOL data), sequence
/// `seq`, `bcast` = DA is broadcast/multicast.
fn tx_desc(frame_len: usize, seq: u16, bcast: bool, qsel: u32) -> [u8; TX_DESC_SIZE] {
    let mut d = [0u8; TX_DESC_SIZE];
    d[0..2].copy_from_slice(&(frame_len as u16).to_le_bytes());       // pkt_size
    d[2] = TX_DESC_SIZE as u8;                                        // pkt_offset = 32
    d[3] = 0x80 | 0x08 | 0x04 | if bcast { 0x01 } else { 0 };         // OWN|FSG|LSG[|BMC]
    d[4..8].copy_from_slice(&(qsel << 8).to_le_bytes());             // txdw1: QSEL<<8
    d[8..12].copy_from_slice(&0x0301_0000u32.to_le_bytes());          // txdw2: ANT_A|ANT_B|AGG_BREAK
    d[12..16].copy_from_slice(&(((seq as u32) & 0xFFF) << 16).to_le_bytes()); // txdw3: seq
    d[16..20].copy_from_slice(&0x0000_0100u32.to_le_bytes());         // txdw4: USE_DRIVER_RATE
    d[20..24].copy_from_slice(&0x001A_0000u32.to_le_bytes());         // txdw5: rate1M|retry6|RL_EN
    d[30..32].copy_from_slice(&0x2000u16.to_le_bytes());              // txdw7: ANT_C
    tx_desc_csum(&mut d);
    d
}

/// Send one 802.11 frame on the bulk-OUT pipe (txdesc32 prepended) on TX queue
/// `qsel`. `seq` must match the frame's header sequence; `bcast` for a
/// broadcast/multicast DA. Returns the bulk completion code (1 = Success).
fn send_frame_q(x: &mut Xhci, st: &mut WifiState, frame: &[u8], seq: u16, bcast: bool, qsel: u32) -> u8 {
    let total = TX_DESC_SIZE + frame.len();
    if total > 4096 { return 0; }
    let desc = tx_desc(frame.len(), seq, bcast, qsel);
    unsafe {
        let p = st.data.virt.as_ptr::<u8>() as *mut u8;
        core::ptr::copy_nonoverlapping(desc.as_ptr(), p, TX_DESC_SIZE);
        core::ptr::copy_nonoverlapping(frame.as_ptr(), p.add(TX_DESC_SIZE), frame.len());
    }
    match crate::usb::xhci::bulk::bulk_xfer(
        x, st.slot_id, st.dci_out, &st.ring_out, &mut st.enq_out, &mut st.cyc_out,
        st.data.phys.as_u64(), total as u32,
    ) {
        Some((code, _)) => code,
        None => 0,
    }
}

/// Send a management frame (probe/auth/assoc) on the MGMT TX queue.
pub fn send_frame(x: &mut Xhci, st: &mut WifiState, frame: &[u8], seq: u16, bcast: bool) -> u8 {
    send_frame_q(x, st, frame, seq, bcast, QSEL_MGMT)
}

/// Send an 802.11 data frame (EAPOL) on the best-effort TX queue.
fn send_data_frame(x: &mut Xhci, st: &mut WifiState, frame: &[u8], seq: u16) -> u8 {
    send_frame_q(x, st, frame, seq, false, QSEL_BE)
}

/// Next 802.11 sequence number (12-bit, wrapping).
fn next_seq(st: &mut WifiState) -> u16 {
    let s = st.tx_seq & 0x0FFF;
    st.tx_seq = (st.tx_seq.wrapping_add(1)) & 0x0FFF;
    s
}

/// Receive one frame on the bulk-IN pipe and copy the 802.11 payload (after the
/// RX descriptor + driver-info) into `out`. Returns the payload length, or None.
pub fn recv_frame(x: &mut Xhci, st: &mut WifiState, out: &mut [u8], timeout_ms: u64) -> Option<usize> {
    let (code, residual) = crate::usb::xhci::bulk::bulk_xfer_timeout(
        x, st.slot_id, st.dci_in, &st.ring_in, &mut st.enq_in, &mut st.cyc_in,
        st.data.phys.as_u64(), 4096, timeout_ms,
    )?;
    if code != 1 && code != 13 { return None; }
    let got = (4096u32.saturating_sub(residual)) as usize;
    if got < RX_DESC_SIZE { return None; }
    let rd = |o: usize| unsafe { read_volatile(st.data.virt.as_ptr::<u8>().add(o)) };
    // rxdesc16 dw0: pkt_len [0:13]; drvinfo_sz [16:19] (8-byte units); shift [24:25].
    // Frame starts after the 24-byte descriptor + drvinfo + shift.
    let dw0 = (rd(0) as u32) | ((rd(1) as u32) << 8) | ((rd(2) as u32) << 16) | ((rd(3) as u32) << 24);
    let pkt_len = (dw0 & 0x3FFF) as usize;
    let drvinfo = ((dw0 >> 16) & 0x0F) as usize * 8;
    let shift = ((dw0 >> 24) & 0x03) as usize;
    let start = RX_DESC_SIZE + drvinfo + shift;
    let end = (start + pkt_len).min(got);
    if end <= start { return None; }
    let n = (end - start).min(out.len());
    unsafe {
        core::ptr::copy_nonoverlapping(st.data.virt.as_ptr::<u8>().add(start), out.as_mut_ptr(), n);
    }
    Some(n)
}

/// Active scan on the *current* channel: broadcast a probe-request, then poll
/// for beacons/probe-responses and collect APs. Channel hopping needs RF/PHY
/// init + channel-set (SP3c/SP4) which are NOT yet implemented, so this only
/// sees APs on whatever channel the chip powers up on. UNVERIFIED on hardware.
/// Tune the radio to a 2.4 GHz channel (config_channel, 20 MHz): BW_OPMODE +
/// FPGA RF-mode bandwidth bits + RF6052 MODE_AG (0x18) channel + BW fields.
fn config_channel(x: &mut Xhci, dev: &mut UsbDevice, channel: u8) {
    if let Some(v) = reg_read8(x, dev, REG_BW_OPMODE) {
        reg_write8(x, dev, REG_BW_OPMODE, v | BW_OPMODE_20MHZ);
    }
    if let Some(v) = reg_read32(x, dev, REG_FPGA0_RF_MODE) {
        reg_write32(x, dev, REG_FPGA0_RF_MODE, v & !FPGA_RF_MODE_BW);
    }
    if let Some(v) = reg_read32(x, dev, REG_FPGA1_RF_MODE) {
        reg_write32(x, dev, REG_FPGA1_RF_MODE, v & !FPGA_RF_MODE_BW);
    }
    let v = read_rfreg(x, dev, RF6052_REG_MODE_AG);
    write_rfreg(x, dev, RF6052_REG_MODE_AG, (v & !MODE_AG_CHANNEL_MASK) | (channel as u32 & MODE_AG_CHANNEL_MASK));
    let v = read_rfreg(x, dev, RF6052_REG_MODE_AG);
    write_rfreg(x, dev, RF6052_REG_MODE_AG, (v & !MODE_AG_BW_MASK) | MODE_AG_BW_20MHZ);
}

/// SP3c-6: passive scan. Hop the 2.4 GHz channels, poll bulk-IN for beacons /
/// probe-responses (short timeout so idle channels don't stall), parse + collect
/// unique APs. Logs each discovered SSID. Needs the radio brought up (SP3c-3/4)
/// and the bulk rings (configure_endpoints).
pub fn scan(x: &mut Xhci, dev: &mut UsbDevice, st: &mut WifiState) -> alloc::vec::Vec<ieee80211::ScanResult> {
    use alloc::vec::Vec;
    let mut results: Vec<ieee80211::ScanResult> = Vec::new();
    let mut buf = [0u8; 2048];
    let mut logged_tx = false;
    for &ch in &[1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13] {
        config_channel(x, dev, ch);
        crate::boot::clock::mdelay(15); // let the synthesizer settle
        // Active scan: broadcast a probe-request (SP-WIFI-1 — proves TX works).
        let seq = next_seq(st);
        let probe = ieee80211::build_probe_request(st.mac, [0xFF; 6], &[], seq);
        let code = send_frame(x, st, &probe, seq, true);
        if !logged_tx {
            crate::binfo!("wifi", "tx probe code={} (1=ok)", code);
            logged_tx = true;
        }
        for _ in 0..12 {
            match recv_frame(x, st, &mut buf, 40) {
                Some(n) => {
                    if let Some(ap) = ieee80211::parse_beacon(&buf[..n]) {
                        if !ap.ssid.is_empty() && !results.iter().any(|r| r.bssid == ap.bssid) {
                            crate::binfo!("wifi", "scan: ssid='{}' ch={} sec={}",
                                ap.ssid, ap.channel, ap.security.as_str());
                            results.push(ap);
                        }
                    }
                }
                None => break, // nothing queued this dwell — next channel
            }
        }
    }
    crate::binfo!("wifi", "scan done: {} AP(s)", results.len());
    results
}

/// `wifiscan` command entry. Lazily brings the chip up (power-on + firmware +
/// MAC/BB/RF) on the first call, then runs a passive scan. Formats the APs as
/// `ssid\tchannel\tsecurity\n` into `out` and returns the byte count (0 = no
/// device / no APs). Follows the MSC pattern: copy device + state out of the
/// registry, run transfers holding only the controllers lock, write cursors back.
pub fn run_scan(out: &mut [u8]) -> usize {
    use core::fmt::Write as _;
    let (ctrl, slot) = match crate::usb::registry::first_wifi_slot() { Some(s) => s, None => return 0 };
    let cell = match crate::usb::CTRLS.get() { Some(c) => c, None => return 0 };
    let mut g = cell.lock();
    let x = match g.get_mut(ctrl as usize) { Some(x) => x, None => return 0 };
    let mut dev = match crate::usb::registry::dev_copy(ctrl, slot) { Some(d) => d, None => return 0 };
    let mut st = match crate::usb::registry::wifi_state(ctrl, slot) { Some(s) => s, None => return 0 };

    ensure_radio_up(x, &mut dev, &mut st);
    let results = scan(x, &mut dev, &mut st);
    // Persist advanced EP0 cursors + ring cursors + the radio_up latch.
    crate::usb::registry::set_dev(ctrl, slot, dev);
    crate::usb::registry::set_wifi_state(ctrl, slot, st);

    let mut s = alloc::string::String::new();
    for r in &results {
        let _ = write!(s, "{}\t{}\t{}\n", r.ssid, r.channel, r.security.as_str());
    }
    let bytes = s.as_bytes();
    let n = bytes.len().min(out.len());
    out[..n].copy_from_slice(&bytes[..n]);
    n
}

/// Lazily bring the chip up on the first scan/connect: power-on + firmware +
/// MAC/BB/RF, then read the station MAC from EFUSE into REG_MACID. The
/// `radio_up` latch keeps subsequent calls fast.
fn ensure_radio_up(x: &mut Xhci, dev: &mut UsbDevice, st: &mut WifiState) {
    if st.radio_up { return; }
    crate::binfo!("wifi", "bring-up (power-on + firmware + radio)");
    bring_up(x, dev, st.nr_out_eps);
    bring_up_radio(x, dev);
    // Station MAC comes from EFUSE; program REG_MACID so the chip's RX address
    // filter matches our SA. Fall back to whatever REG_MACID already holds.
    match read_efuse_mac(x, dev) {
        Some(mac) => {
            for i in 0..6u16 { reg_write8(x, dev, REG_MACID + i, mac[i as usize]); }
            st.mac = mac;
        }
        None => {
            for i in 0..6u16 { st.mac[i as usize] = reg_read8(x, dev, REG_MACID + i).unwrap_or(0); }
        }
    }
    crate::binfo!("wifi", "mac={:02x?}", st.mac);
    st.radio_up = true;
}

// ── SP-WIFI-2: STA association ───────────────────────────────────────────────

/// Outcome of an association attempt (up to assoc — the 4-way handshake and key
/// install are SP-WIFI-3/4). `None` status = no response received.
pub struct ConnectOutcome {
    pub found: bool,
    pub auth_status: Option<u16>,
    pub assoc_status: Option<u16>,
    pub aid: u16,
    /// WPA2 4-way handshake result: None = not attempted (open net / no password),
    /// Some(true) = complete + keys installed, Some(false) = failed.
    pub four_way: Option<bool>,
}

/// Set port-0 operating mode to STATION (REG_MSR link type).
fn set_opmode_sta(x: &mut Xhci, dev: &mut UsbDevice) {
    let v = reg_read8(x, dev, REG_MSR).unwrap_or(0) & 0x0c;
    reg_write8(x, dev, REG_MSR, v | MSR_LINKTYPE_STATION);
}

/// Program the BSSID of the AP we're joining (REG_BSSID, 6 bytes).
fn set_bssid_reg(x: &mut Xhci, dev: &mut UsbDevice, bssid: [u8; 6]) {
    for i in 0..6u16 { reg_write8(x, dev, REG_BSSID + i, bssid[i as usize]); }
}

/// Send one gen2 H2C mailbox command word. Polls REG_HMTFR for the box free
/// (bounded), writes the 4-byte mailbox, advances the round-robin index.
fn h2c_cmd(x: &mut Xhci, dev: &mut UsbDevice, st: &mut WifiState, word: u32) {
    let nr = st.h2c_mbox % H2C_MAX_MBOX;
    for _ in 0..100 {
        if reg_read8(x, dev, REG_HMTFR).unwrap_or(0xff) & (1 << nr) == 0 { break; }
    }
    reg_write32(x, dev, REG_HMBOX_0 + (nr as u16) * 4, word);
    st.h2c_mbox = (nr + 1) % H2C_MAX_MBOX;
}

/// H2C media-status report (rtl8xxxu_gen2_report_connect): cmd 0x01, parm =
/// connect(bit0) | role<<4, macid 0 (= the AP, from the STA's perspective).
fn report_connect(x: &mut Xhci, dev: &mut UsbDevice, st: &mut WifiState, connect: bool) {
    let parm = (if connect { 1u32 } else { 0 }) | ((H2C_MACID_ROLE_AP as u32) << 4);
    let word = (H2C_MEDIA_STATUS_RPT as u32) | (parm << 8); // macid 0 in byte 2
    h2c_cmd(x, dev, st, word);
}

/// Write a 16-byte CCMP key into the chip's security CAM (rtl8xxxu_cam_write):
/// 6 dwords per entry at `hw_idx`, `keyidx` = the 802.11 key index, `mac` = the
/// peer address (BSSID), `group` selects the group-key flag.
fn cam_write_key(x: &mut Xhci, dev: &mut UsbDevice, hw_idx: u8, keyidx: u8,
                 key: &[u8; 16], mac: [u8; 6], group: bool) {
    let addr = (hw_idx as u32) << CAM_CMD_KEY_SHIFT;
    let mut ctrl = (CCMP_CIPHER_NIBBLE << 2) | (keyidx as u32) | CAM_WRITE_VALID;
    if group { ctrl |= 1 << 6; }
    for j in (0..6u32).rev() {
        let val32 = match j {
            0 => ctrl | ((mac[0] as u32) << 16) | ((mac[1] as u32) << 24),
            1 => (mac[2] as u32) | ((mac[3] as u32) << 8)
                 | ((mac[4] as u32) << 16) | ((mac[5] as u32) << 24),
            _ => {
                let i = ((j - 2) * 4) as usize;
                (key[i] as u32) | ((key[i + 1] as u32) << 8)
                | ((key[i + 2] as u32) << 16) | ((key[i + 3] as u32) << 24)
            }
        };
        reg_write32(x, dev, REG_CAM_WRITE, val32);
        reg_write32(x, dev, REG_CAM_CMD, CAM_CMD_POLLING | CAM_CMD_WRITE | (addr + j));
        crate::boot::clock::udelay(100);
    }
}

/// Turn on HW security (rtl8xxxu set_key prologue): CR security-enable +
/// SEC_CFG TX/RX encrypt + default-key for uni and broadcast.
fn enable_hw_security(x: &mut Xhci, dev: &mut UsbDevice) {
    if let Some(v) = reg_read16(x, dev, REG_CR) {
        reg_write16(x, dev, REG_CR, v | CR_SECURITY_ENABLE);
    }
    reg_write8(x, dev, REG_SECURITY_CFG, SEC_CFG_VAL);
}

/// Receive the next EAPOL-Key frame from the AP matching `want`/`mask` on the
/// key_info bits, within `timeout_ms` of WALL-CLOCK time (not a frame count — a
/// busy channel floods beacons that would otherwise burn an iteration budget
/// before the EAPOL frame arrives). Logs the key_info of every EAPOL seen.
fn recv_eapol(x: &mut Xhci, st: &mut WifiState, buf: &mut [u8],
              want: u16, mask: u16, timeout_ms: u64, label: &str) -> Option<eapol::KeyFrame> {
    let deadline = crate::boot::clock::elapsed_ms() + timeout_ms;
    let mut total = 0u32;
    let mut data = 0u32;
    while crate::boot::clock::elapsed_ms() < deadline {
        if let Some(n) = recv_frame(x, st, buf, 50) {
            total += 1;
            if n > 0 && buf[0] & 0x0C == 0x08 { data += 1; } // data-type frame
            if let Some(payload) = ieee80211::parse_eapol_data(&buf[..n]) {
                if let Some(kf) = eapol::parse(payload) {
                    crate::binfo!("wifi", "4way {}: eapol key_info={:#06x}", label, kf.key_info);
                    if kf.key_info & mask == want {
                        return Some(kf);
                    }
                }
            }
        }
    }
    crate::binfo!("wifi", "4way {}: timeout (rx total={} data={})", label, total, data);
    None
}

/// Run the WPA2-PSK 4-way handshake after association. Returns the installed
/// PTK + (GTK key-index, GTK) on success. Drives EAPOL over 802.11 data frames;
/// the crypto is `wpa2`. A MIC mismatch on msg-3 means the wrong passphrase.
fn four_way_handshake(x: &mut Xhci, dev: &mut UsbDevice, st: &mut WifiState,
                      pmk: &[u8; 32], bssid: [u8; 6]) -> Option<(wpa2::Ptk, u8, alloc::vec::Vec<u8>)> {
    let mut buf = [0u8; 600];

    // msg1 (AP→STA): ANonce. key_info: ACK set, MIC clear.
    let m1 = match recv_eapol(x, st, &mut buf, eapol::KEY_INFO_ACK,
                              eapol::KEY_INFO_ACK | eapol::KEY_INFO_MIC, 3000, "m1") {
        Some(k) => k,
        None => { crate::bwarn!("wifi", "4way: no msg1"); return None; }
    };
    crate::binfo!("wifi", "4way: msg1 ok (anonce)");
    let anonce = m1.nonce;

    // Generate SNonce, derive PTK.
    let mut snonce = [0u8; 32];
    crate::rng::fill(&mut snonce);
    let ptk = wpa2::derive_ptk(pmk, &bssid, &st.mac, &anonce, &snonce);

    // msg2 (STA→AP): SNonce + our RSN IE + MIC.
    let ki2 = eapol::KEY_DESC_VERSION_2 | eapol::KEY_INFO_KEY_TYPE | eapol::KEY_INFO_MIC;
    let rsn = ieee80211::rsn_ie_wpa2_psk();
    let mut m2 = eapol::build(ki2, 16, &m1.replay_counter, &snonce, &rsn);
    let mic = wpa2::eapol_mic(&ptk.kck, &m2);
    m2[eapol::MIC_OFFSET..eapol::MIC_OFFSET + eapol::MIC_LEN].copy_from_slice(&mic);
    let seq = next_seq(st);
    let c2 = send_data_frame(x, st, &ieee80211::build_eapol_data(st.mac, bssid, &m2, seq), seq);
    crate::binfo!("wifi", "4way: msg2 sent code={}", c2);

    // msg3 (AP→STA): MIC + ENCRYPTED key data (GTK).
    let m3 = match recv_eapol(x, st, &mut buf,
                              eapol::KEY_INFO_MIC | eapol::KEY_INFO_ENCRYPTED,
                              eapol::KEY_INFO_MIC | eapol::KEY_INFO_ENCRYPTED, 3000, "m3") {
        Some(k) => k,
        None => { crate::bwarn!("wifi", "4way: no msg3"); return None; }
    };
    // Verify the msg3 MIC over the exact received bytes with the MIC field
    // zeroed (proves the PMK / passphrase).
    let mut m3_chk = m3.raw.clone();
    if m3_chk.len() < eapol::MIC_OFFSET + eapol::MIC_LEN { return None; }
    for b in &mut m3_chk[eapol::MIC_OFFSET..eapol::MIC_OFFSET + eapol::MIC_LEN] { *b = 0; }
    if wpa2::eapol_mic(&ptk.kck, &m3_chk) != m3.mic {
        crate::bwarn!("wifi", "4way: msg3 MIC mismatch (wrong passphrase?)");
        return None;
    }
    crate::binfo!("wifi", "4way: msg3 mic ok");
    // Unwrap the encrypted key data with the KEK → extract the GTK KDE.
    let plain = match wpa2::aes_unwrap(&ptk.kek, &m3.key_data) {
        Some(p) => p,
        None => { crate::bwarn!("wifi", "4way: gtk unwrap failed"); return None; }
    };
    let (gtk_idx, gtk) = match eapol::extract_gtk(&plain) {
        Some(g) => g,
        None => { crate::bwarn!("wifi", "4way: no gtk kde in msg3"); return None; }
    };

    // msg4 (STA→AP): MIC + SECURE, empty key data.
    let ki4 = eapol::KEY_DESC_VERSION_2 | eapol::KEY_INFO_KEY_TYPE
            | eapol::KEY_INFO_MIC | eapol::KEY_INFO_SECURE;
    let mut m4 = eapol::build(ki4, 16, &m3.replay_counter, &[0u8; 32], &[]);
    let mic4 = wpa2::eapol_mic(&ptk.kck, &m4);
    m4[eapol::MIC_OFFSET..eapol::MIC_OFFSET + eapol::MIC_LEN].copy_from_slice(&mic4);
    let seq = next_seq(st);
    send_data_frame(x, st, &ieee80211::build_eapol_data(st.mac, bssid, &m4, seq), seq);

    crate::binfo!("wifi", "4-way complete (gtk idx={} len={})", gtk_idx, gtk.len());
    Some((ptk, gtk_idx, gtk))
}

/// Connect to `ssid`: open-system auth → WPA2 association → (if `password` set)
/// the WPA2-PSK 4-way handshake + CCMP key install in the HW CAM.
pub fn connect(x: &mut Xhci, dev: &mut UsbDevice, st: &mut WifiState,
               ssid: &[u8], password: &[u8]) -> ConnectOutcome {
    let mut out = ConnectOutcome {
        found: false, auth_status: None, assoc_status: None, aid: 0, four_way: None,
    };

    // 1. Locate the AP (BSSID + channel) via a scan.
    let aps = scan(x, dev, st);
    let (bssid, channel) = match aps.iter().find(|r| r.ssid.as_bytes() == ssid) {
        Some(a) => (a.bssid, a.channel),
        None => { crate::bwarn!("wifi", "connect: ssid not found"); return out; }
    };
    out.found = true;
    crate::binfo!("wifi", "connect: ap {:02x?} ch={}", bssid, channel);

    // Pre-compute the PMK now (PBKDF2 is slow) so the 4-way can start listening
    // for msg1 the instant association completes, before beacons flood the RX FIFO.
    let pmk = if !password.is_empty() { Some(wpa2::pmk(password, ssid)) } else { None };

    // 2. Tune to the AP channel, set STA opmode + BSSID.
    config_channel(x, dev, channel);
    crate::boot::clock::mdelay(10);
    set_opmode_sta(x, dev);
    set_bssid_reg(x, dev, bssid);

    let mut buf = [0u8; 512];

    // 3. Open-system authentication.
    'auth: for _ in 0..4 {
        let seq = next_seq(st);
        let frame = ieee80211::build_auth_request(st.mac, bssid, seq);
        send_frame(x, st, &frame, seq, false);
        for _ in 0..20 {
            if let Some(n) = recv_frame(x, st, &mut buf, 50) {
                if let Some(status) = ieee80211::parse_auth_response(&buf[..n]) {
                    out.auth_status = Some(status);
                    break 'auth;
                }
            }
        }
    }
    match out.auth_status {
        Some(0) => crate::binfo!("wifi", "auth ok"),
        Some(s) => { crate::bwarn!("wifi", "auth rejected status={}", s); return out; }
        None    => { crate::bwarn!("wifi", "auth no response"); return out; }
    }

    // 4. Association (carries the WPA2-PSK RSN IE).
    'assoc: for _ in 0..4 {
        let seq = next_seq(st);
        let frame = ieee80211::build_assoc_request(st.mac, bssid, ssid, seq);
        send_frame(x, st, &frame, seq, false);
        for _ in 0..20 {
            if let Some(n) = recv_frame(x, st, &mut buf, 50) {
                if let Some((status, aid)) = ieee80211::parse_assoc_response(&buf[..n]) {
                    out.assoc_status = Some(status);
                    out.aid = aid;
                    break 'assoc;
                }
            }
        }
    }
    match out.assoc_status {
        Some(0) => crate::binfo!("wifi", "assoc ok aid={}", out.aid),
        Some(s) => { crate::bwarn!("wifi", "assoc rejected status={}", s); return out; }
        None    => { crate::bwarn!("wifi", "assoc no response"); return out; }
    }

    // 5. joinbss: AID register + H2C media-connect (firmware enables rate control).
    reg_write8(x, dev, REG_BCN_MAX_ERR, 0xff);
    reg_write16(x, dev, REG_BCN_PSR_RPT, 0xc000 | out.aid);
    report_connect(x, dev, st, true);
    crate::binfo!("wifi", "media-connect sent (aid={})", out.aid);

    // 6. WPA2-PSK 4-way handshake + CCMP key install (skipped for an open net).
    if let Some(pmk) = pmk {
        match four_way_handshake(x, dev, st, &pmk, bssid) {
            Some((ptk, gtk_idx, gtk)) if gtk.len() >= 16 => {
                enable_hw_security(x, dev);
                cam_write_key(x, dev, 0, 0, &ptk.tk, bssid, false); // pairwise (PTK TK)
                let mut gk = [0u8; 16];
                gk.copy_from_slice(&gtk[..16]);
                cam_write_key(x, dev, 1, gtk_idx, &gk, bssid, true); // group (GTK)
                crate::binfo!("wifi", "cam: ptk+gtk installed (gtk idx={})", gtk_idx);
                out.four_way = Some(true);
            }
            _ => {
                crate::bwarn!("wifi", "4-way failed");
                out.four_way = Some(false);
            }
        }
    }
    out
}

/// `wificonnect` command entry. Lazily brings the chip up, then runs the
/// open-auth → WPA2-assoc sequence to `ssid`. Writes a one-line status into
/// `out` and returns the byte count (0 = no device).
pub fn run_connect(ssid: &[u8], password: &[u8], out: &mut [u8]) -> usize {
    use core::fmt::Write as _;
    let (ctrl, slot) = match crate::usb::registry::first_wifi_slot() { Some(s) => s, None => return 0 };
    let cell = match crate::usb::CTRLS.get() { Some(c) => c, None => return 0 };
    let mut g = cell.lock();
    let x = match g.get_mut(ctrl as usize) { Some(x) => x, None => return 0 };
    let mut dev = match crate::usb::registry::dev_copy(ctrl, slot) { Some(d) => d, None => return 0 };
    let mut st = match crate::usb::registry::wifi_state(ctrl, slot) { Some(s) => s, None => return 0 };

    ensure_radio_up(x, &mut dev, &mut st);
    let o = connect(x, &mut dev, &mut st, ssid, password);
    crate::usb::registry::set_dev(ctrl, slot, dev);
    crate::usb::registry::set_wifi_state(ctrl, slot, st);

    let mut s = alloc::string::String::new();
    if !o.found {
        let _ = write!(s, "ssid not found\n");
    } else {
        let auth = match o.auth_status { Some(0) => "ok", Some(_) => "rejected", None => "no-response" };
        let assoc = match o.assoc_status { Some(0) => "ok", Some(_) => "rejected", None => "no-response" };
        let key = match o.four_way { Some(true) => "ok", Some(false) => "failed", None => "skipped" };
        let _ = write!(s, "auth={} assoc={} aid={} 4way={}\n", auth, assoc, o.aid, key);
    }
    let bytes = s.as_bytes();
    let n = bytes.len().min(out.len());
    out[..n].copy_from_slice(&bytes[..n]);
    n
}
