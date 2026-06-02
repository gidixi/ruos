//! USB EP0 control transfers over the xHCI transfer ring.
use crate::usb::xhci::Xhci;
use crate::usb::device::UsbDevice;
use crate::memory::dma::DmaRegion;

/// A USB SETUP packet (8 bytes), split into fields.
#[derive(Clone, Copy)]
pub struct Setup {
    pub req_type: u8,  // bmRequestType
    pub request:  u8,  // bRequest
    pub value:    u16, // wValue
    pub index:    u16, // wIndex
    pub length:   u16, // wLength
}

/// Control-IN transfer: push Setup + Data(IN) + Status(OUT) onto the EP0
/// transfer ring, ring the slot doorbell (target = DCI 1), then poll for a
/// Transfer Event (TRB type 32). Returns bytes actually transferred on success.
///
/// `buf` must be a DMA region large enough to hold `s.length` bytes.
pub fn control_in(x: &mut Xhci, dev: &mut UsbDevice, s: Setup, buf: &DmaRegion) -> Option<u16> {
    // ── Setup Stage TRB (type 2) ────────────────────────────────────────────
    // word0: bmRequestType | bRequest<<8 | wValue<<16
    // word1: wIndex | wLength<<16
    // word2: TRB Transfer Length = 8 (always for setup)
    // word3: IDT(6) | type(10..15)=2 | TRT(16..17)=3(IN) — cycle added by enqueue_xfer
    let w0 = (s.req_type as u32)
        | ((s.request as u32) << 8)
        | ((s.value as u32) << 16);
    let w1 = (s.index as u32) | ((s.length as u32) << 16);
    let setup = [w0, w1, 8u32, (1 << 6) | (2 << 10) | (3 << 16)];

    // ── Data Stage TRB (type 3, DIR=IN) ────────────────────────────────────
    // word0/1: data buffer physical address (lo/hi)
    // word2: transfer length (wLength)
    // word3: type(10..15)=3 | DIR(16)=1(IN) — cycle added by enqueue_xfer
    let dphys = buf.phys.as_u64();
    let data = [
        (dphys & 0xFFFF_FFFF) as u32,
        (dphys >> 32) as u32,
        s.length as u32,
        (3 << 10) | (1 << 16),
    ];

    // ── Status Stage TRB (type 4, DIR=OUT, IOC) ────────────────────────────
    // For a control-IN transfer, the status stage is OUT (DIR=0).
    // IOC (bit5) = 1 so the HC fires a Transfer Event we can poll.
    // word3: IOC(5) | type(10..15)=4 | DIR(16)=0 — cycle added by enqueue_xfer
    let status = [0u32, 0u32, 0u32, (1 << 5) | (4 << 10)];

    // Push all three TRBs — doorbell is rung AFTER all three are enqueued.
    crate::usb::xhci::ring::enqueue_xfer(
        &dev.ep0_ring, &mut dev.ep0_enqueue, &mut dev.ep0_cycle, setup,
    );
    crate::usb::xhci::ring::enqueue_xfer(
        &dev.ep0_ring, &mut dev.ep0_enqueue, &mut dev.ep0_cycle, data,
    );
    crate::usb::xhci::ring::enqueue_xfer(
        &dev.ep0_ring, &mut dev.ep0_enqueue, &mut dev.ep0_cycle, status,
    );

    // ── Ring EP0 doorbell (slot doorbell, target DCI 1 = EP0) ──────────────
    x.regs.doorbell.update_volatile_at(dev.slot_id as usize, |d| {
        d.set_doorbell_target(1);
    });

    // ── Wait for Transfer Event (type 32) — up to 200 ms ──────────────────
    // Foreign events (port status change, other slots' transfers) are routed
    // through the central dispatcher by `wait_for`, not dropped.
    let ev = crate::usb::xhci::event::wait_for(x, 200, |w| {
        crate::usb::xhci::ring::trb_type(w) == 32
    })?;
    let code = crate::usb::xhci::ring::completion_code(&ev);
    // Code 1 = Success, code 13 = Short Packet (also OK for IN)
    if code != 1 && code != 13 {
        crate::bwarn!("usb", "control_in xfer event code={}", code);
        return None;
    }
    // word2 bits 0..23 = residual transfer length
    let residual = ev[2] & 0x00FF_FFFF;
    Some(s.length.saturating_sub(residual as u16))
}

/// Control transfer with no Data stage (e.g. SET_CONFIGURATION, SET_PROTOCOL).
/// Pushes Setup(TRT=No Data) + Status(IN, IOC), rings EP0 doorbell, waits.
pub fn control_out(x: &mut Xhci, dev: &mut UsbDevice, s: Setup) -> bool {
    let w0 = (s.req_type as u32) | ((s.request as u32) << 8) | ((s.value as u32) << 16);
    let w1 = (s.index as u32) | ((s.length as u32) << 16);
    // Setup: IDT(bit6) | type 2 | TRT=0 (No Data Stage). word3 bits16..17 = 0.
    let setup = [w0, w1, 8u32, (1 << 6) | (2 << 10)];
    // Status for a no-data / OUT transfer is IN (DIR=1), with IOC.
    let status = [0u32, 0u32, 0u32, (4 << 10) | (1 << 5) | (1 << 16)];
    crate::usb::xhci::ring::enqueue_xfer(
        &dev.ep0_ring, &mut dev.ep0_enqueue, &mut dev.ep0_cycle, setup,
    );
    crate::usb::xhci::ring::enqueue_xfer(
        &dev.ep0_ring, &mut dev.ep0_enqueue, &mut dev.ep0_cycle, status,
    );
    x.regs.doorbell.update_volatile_at(dev.slot_id as usize, |d| {
        d.set_doorbell_target(1);
    });
    let ev = match crate::usb::xhci::event::wait_for(x, 100, |w| {
        crate::usb::xhci::ring::trb_type(w) == 32
    }) {
        Some(e) => e,
        None => {
            crate::bwarn!("usb", "control_out timeout");
            return false;
        }
    };
    let code = crate::usb::xhci::ring::completion_code(&ev);
    if code != 1 {
        crate::bwarn!("usb", "control_out code={}", code);
        return false;
    }
    true
}

// ── Hub class requests (USB 2.0 §11.24) ─────────────────────────────────────
// These operate on the hub's own EP0 (`dev` = the hub's UsbDevice). The class
// constants live here so the hub driver builds Setup packets symbolically.

/// GET_DESCRIPTOR(Hub): bmRequestType=0xA0 (Dev→Host,Class,Device), wValue=0x2900
/// (descriptor type 0x29, index 0). 71 = max hub-descriptor length (255-port DR).
pub fn get_hub_descriptor(x: &mut Xhci, dev: &mut UsbDevice, buf: &DmaRegion) -> Option<u16> {
    control_in(
        x,
        dev,
        Setup { req_type: 0xA0, request: 6, value: 0x2900, index: 0, length: 71 },
        buf,
    )
}

/// GET_STATUS(port): bmRequestType=0xA3 (Dev→Host,Class,Other), 4 data bytes =
/// wPortStatus (LE u16 @0) + wPortChange (LE u16 @2). Returns (status, change).
pub fn get_port_status(
    x: &mut Xhci,
    dev: &mut UsbDevice,
    port: u8,
    buf: &DmaRegion,
) -> Option<(u16, u16)> {
    control_in(
        x,
        dev,
        Setup { req_type: 0xA3, request: 0, value: 0, index: port as u16, length: 4 },
        buf,
    )?;
    let p = buf.virt.as_ptr::<u8>();
    // SAFETY: 4-byte read from a DMA page that just received >=4 bytes.
    let st = unsafe { (*p as u16) | ((*p.add(1) as u16) << 8) };
    let ch = unsafe { (*p.add(2) as u16) | ((*p.add(3) as u16) << 8) };
    Some((st, ch))
}

/// SET_FEATURE(port, feature): bmRequestType=0x23 (Host→Dev,Class,Other),
/// bRequest=3, wValue=feature, wIndex=port. No data stage.
pub fn set_port_feature(x: &mut Xhci, dev: &mut UsbDevice, port: u8, feat: u16) -> bool {
    control_out(
        x,
        dev,
        Setup { req_type: 0x23, request: 3, value: feat, index: port as u16, length: 0 },
    )
}

/// CLEAR_FEATURE(port, feature): like SET_FEATURE but bRequest=1.
pub fn clear_port_feature(x: &mut Xhci, dev: &mut UsbDevice, port: u8, feat: u16) -> bool {
    control_out(
        x,
        dev,
        Setup { req_type: 0x23, request: 1, value: feat, index: port as u16, length: 0 },
    )
}
