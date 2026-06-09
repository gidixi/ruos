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
const REG_SYS_FUNC_EN:    u16 = 0x0002; // FEN_CPUEN @ bit2 gates the 8051
const FEN_CPUEN:          u8  = 0x04;
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
    // 1. Enable the 8051 CPU — 16-bit SYS_FUNC.
    if let Some(v) = reg_read16(x, dev, REG_SYS_FUNC_EN) {
        reg_write16(x, dev, REG_SYS_FUNC_EN, v | FEN_CPUEN as u16);
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
    if !ok { return false; }

    // 8. Leave download mode — 16-bit clear of MCU_FW_DL_ENABLE.
    if let Some(v) = reg_read16(x, dev, REG_MCUFWDL) {
        reg_write16(x, dev, REG_MCUFWDL, v & !(MCUFWDL_EN as u16));
    }

    // start_firmware (rtl8xxxu): wait checksum, mark ready, reset 8051, wait init.
    // 1. Poll the checksum-report bit (download complete).
    let mut csum = false;
    for _ in 0..1000 {
        if let Some(v) = reg_read32(x, dev, REG_MCUFWDL) {
            if (v as u8) & MCUFWDL_CSUM_RPT != 0 { csum = true; break; }
        }
    }
    if !csum { crate::bwarn!("wifi", "fw checksum-report timeout"); }
    // 2. Set MCU_FW_DL_READY, clear WINT_INIT_READY (32-bit RMW on MCUFWDL).
    if let Some(v) = reg_read32(x, dev, REG_MCUFWDL) {
        let nv = (v | MCUFWDL_DL_RDY as u32) & !(WINTINI_RDY as u32);
        reg_write32(x, dev, REG_MCUFWDL, nv);
    }
    // 3. reset_8051: unlock RSV_CTRL, then 16-bit toggle of the 8051 enable.
    reg_write8(x, dev, REG_RSV_CTRL, 0x00);
    if let Some(v) = reg_read16(x, dev, REG_SYS_FUNC_EN) {
        reg_write16(x, dev, REG_SYS_FUNC_EN, v & !(FEN_CPUEN as u16));
        reg_write16(x, dev, REG_SYS_FUNC_EN, v | (FEN_CPUEN as u16));
    }
    // 4. Wait for firmware init-ready (WINT_INIT_READY) in the 32-bit MCUFWDL.
    for _ in 0..1000 {
        if let Some(v) = reg_read32(x, dev, REG_MCUFWDL) {
            if (v as u8) & WINTINI_RDY != 0 { crate::binfo!("wifi", "fw ready"); return true; }
        }
    }
    crate::bwarn!("wifi", "fw ready timeout (verify on HW)");
    false
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

/// SP2 bring-up: power-on then upload firmware. ⚠️ UNVERIFIED on hardware — the
/// register transport (SP1) has not yet been confirmed on a real dongle.
pub fn bring_up(x: &mut Xhci, dev: &mut UsbDevice) -> bool {
    if !power_on(x, dev) { return false; }
    download_firmware(x, dev)
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
                }
                5 if in_iface && pos + 7 <= n => {
                    let addr = rd(pos + 2);
                    let attr = rd(pos + 3);
                    let mps = rd16(pos + 4);
                    if attr & 0x03 == 2 { // bulk
                        if addr & 0x80 != 0 { ep_in.get_or_insert((addr, mps)); }
                        else { ep_out.get_or_insert((addr, mps)); }
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
        crate::binfo!("wifi", "rtl8188eu iface={} ep_in=0x{:02x} ep_out=0x{:02x} mps_in={} mps_out={}",
            iface_num, ep_in, ep_out, mps_in, mps_out);

        // SP2: attempt chip bring-up (power-on + firmware upload). Failure does
        // NOT abort the bind — the slot is still registered so later subprojects
        // can retry. UNVERIFIED on hardware.
        if bring_up(x, dev) {
            crate::binfo!("wifi", "bring-up OK (power-on + firmware)");
        } else {
            crate::bwarn!("wifi", "bring-up incomplete (SP2 unverified — see HW log)");
        }

        Some(WifiIface { iface: iface_num, ep_in, ep_out, mps_in, mps_out })
    })();
    crate::memory::dma::dealloc(buf);
    result
}

// ── SP3b: bulk endpoint configuration + frame I/O ────────────────────────────

/// RTL8188EU TX descriptor size (txdesc40) prepended to every outgoing 802.11
/// frame on the bulk-OUT pipe. RX descriptor (rxdesc24) precedes each received
/// frame on bulk-IN. From the rtl8xxxu reference.
const TX_DESC_SIZE: usize = 40;
const RX_DESC_SIZE: usize = 24;
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
    })
}

/// Build the 40-byte RTL TX descriptor for an 802.11 management frame of
/// `frame_len` bytes. Sets packet size, header offset, first/last segment and
/// the management queue. Other fields (rate, macid) left default. From the
/// rtl8xxxu txdesc40 layout — UNVERIFIED on hardware.
fn tx_desc_mgmt(frame_len: usize) -> [u8; TX_DESC_SIZE] {
    let mut d = [0u8; TX_DESC_SIZE];
    // txdw0: pkt_size[0:15] | offset[16:23]=TX_DESC_SIZE | FSG[25] | LSG[26].
    let dw0 = (frame_len as u32 & 0xFFFF)
        | ((TX_DESC_SIZE as u32) << 16)
        | (1 << 25)
        | (1 << 26);
    d[0..4].copy_from_slice(&dw0.to_le_bytes());
    // txdw1: QSEL[8:12] = management queue.
    let dw1 = QSEL_MGMT << 8;
    d[4..8].copy_from_slice(&dw1.to_le_bytes());
    d
}

/// Send one 802.11 frame on the bulk-OUT pipe (TX descriptor prepended). Returns
/// false on a non-success completion or oversize frame.
pub fn send_frame(x: &mut Xhci, st: &mut WifiState, frame: &[u8]) -> bool {
    let total = TX_DESC_SIZE + frame.len();
    if total > 4096 { return false; }
    let desc = tx_desc_mgmt(frame.len());
    unsafe {
        let p = st.data.virt.as_ptr::<u8>() as *mut u8;
        core::ptr::copy_nonoverlapping(desc.as_ptr(), p, TX_DESC_SIZE);
        core::ptr::copy_nonoverlapping(frame.as_ptr(), p.add(TX_DESC_SIZE), frame.len());
    }
    match crate::usb::xhci::bulk::bulk_xfer(
        x, st.slot_id, st.dci_out, &st.ring_out, &mut st.enq_out, &mut st.cyc_out,
        st.data.phys.as_u64(), total as u32,
    ) {
        Some((1, _)) => true,
        _ => false,
    }
}

/// Receive one frame on the bulk-IN pipe and copy the 802.11 payload (after the
/// RX descriptor + driver-info) into `out`. Returns the payload length, or None.
pub fn recv_frame(x: &mut Xhci, st: &mut WifiState, out: &mut [u8]) -> Option<usize> {
    let (code, residual) = crate::usb::xhci::bulk::bulk_xfer(
        x, st.slot_id, st.dci_in, &st.ring_in, &mut st.enq_in, &mut st.cyc_in,
        st.data.phys.as_u64(), 4096,
    )?;
    if code != 1 && code != 13 { return None; }
    let got = (4096u32.saturating_sub(residual)) as usize;
    if got < RX_DESC_SIZE { return None; }
    let rd = |o: usize| unsafe { read_volatile(st.data.virt.as_ptr::<u8>().add(o)) };
    // rxdesc24 dw0: pkt length [0:13]; dw0 bits 24:28 = driver-info size (8-byte
    // units) — payload starts at RX_DESC_SIZE + drvinfo*8.
    let dw0 = (rd(0) as u32) | ((rd(1) as u32) << 8) | ((rd(2) as u32) << 16) | ((rd(3) as u32) << 24);
    let pkt_len = (dw0 & 0x3FFF) as usize;
    let drvinfo = ((dw0 >> 24) & 0x0F) as usize * 8;
    let start = RX_DESC_SIZE + drvinfo;
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
pub fn scan(x: &mut Xhci, st: &mut WifiState, sa: [u8; 6]) -> alloc::vec::Vec<ieee80211::ScanResult> {
    use alloc::vec::Vec;
    let mut results: Vec<ieee80211::ScanResult> = Vec::new();
    let probe = ieee80211::build_probe_request(sa, &[]);
    if !send_frame(x, st, &probe) {
        crate::bwarn!("wifi", "scan: probe-request TX failed");
    }
    let mut buf = [0u8; 2048];
    for _ in 0..32 {
        if let Some(n) = recv_frame(x, st, &mut buf) {
            if let Some(ap) = ieee80211::parse_beacon(&buf[..n]) {
                if !results.iter().any(|r| r.bssid == ap.bssid) {
                    crate::binfo!("wifi", "scan: ssid='{}' ch={} sec={}",
                        ap.ssid, ap.channel, ap.security.as_str());
                    results.push(ap);
                }
            }
        }
    }
    results
}
