//! USB HID boot device (keyboard or mouse): shared interrupt-IN endpoint setup.

use crate::usb::xhci::Xhci;
use crate::usb::device::UsbDevice;
use crate::memory::dma::DmaRegion;

/// A detected HID boot interface's interrupt-IN endpoint. `proto` is the HID boot
/// protocol from the interface descriptor: 1 = keyboard, 2 = mouse.
#[derive(Clone, Copy)]
pub struct HidBootEndpoint {
    pub iface:      u8,   // bInterfaceNumber
    pub ep_addr:    u8,   // bEndpointAddress (bit7=IN, low4=EP number)
    pub max_packet: u16,  // wMaxPacketSize
    pub interval:   u8,   // bInterval
    pub proto:      u8,   // bInterfaceProtocol (1=keyboard, 2=mouse)
}

/// Running state for a configured HID boot endpoint: its interrupt transfer ring
/// + report buffer + last report. `report_len` is how many bytes each Normal TRB
/// requests (8 for a boot keyboard, the endpoint's wMaxPacketSize for a mouse).
pub struct HidState {
    pub slot_id:      u8,
    pub dci:          u8,
    pub int_ring:     DmaRegion,
    pub int_enqueue:  usize,
    pub int_cycle:    bool,
    pub report:       DmaRegion,
    pub report_len:   u32,
    pub prev:         [u8; 8],
}

/// Convert a USB endpoint `bInterval` into the xHCI Endpoint Context `Interval`
/// field, whose period is `2^Interval` microframes (125 µs each).
///
/// The encoding of `bInterval` depends on the device speed, and getting it wrong
/// is silent: the endpoint still configures, it just never gets serviced.
///
/// * High-speed (`speed==3`) / SuperSpeed (`4`) interrupt: `bInterval` is already
///   a microframe exponent in `1..=16` (period `2^(bInterval-1)` microframes),
///   so `Interval = bInterval - 1`.
/// * Full-speed (`1`) / Low-speed (`2`) interrupt: `bInterval` is in **frames**
///   (1 ms = 8 microframes), range `1..=255`. Period in microframes is
///   `bInterval * 8`, so `Interval = floor(log2(bInterval * 8))`.
///
/// Passing the raw `bInterval` for a Full/Low-speed device (e.g. `24`) yields a
/// `2^24`-microframe period — minutes between polls — which is why a real
/// keyboard enumerated but never delivered a report.
fn xhci_interval(speed: u8, b_interval: u8) -> u8 {
    match speed {
        3 | 4 => b_interval.saturating_sub(1).min(15),
        _ => {
            let microframes = (b_interval.max(1) as u32) * 8;
            // floor(log2(microframes)) — microframes >= 8, so this is >= 3.
            let exp = 31 - microframes.leading_zeros();
            exp.clamp(3, 15) as u8
        }
    }
}

/// Configure the keyboard's interrupt-IN endpoint, set boot protocol, and
/// queue the first Normal TRB to receive a report. Returns the running state.
pub fn configure_endpoint(
    x: &mut Xhci,
    dev: &mut UsbDevice,
    kb: &HidBootEndpoint,
) -> Option<HidState> {
    use ::xhci::context::{
        Input32Byte, Input64Byte,
        InputHandler,
        EndpointType,
    };

    // DCI for interrupt-IN ep: 2 * (ep_number) + 1 (IN direction).
    // ep_addr 0x81 → ep_number = 0x81 & 0x0F = 1 → DCI = 3.
    let dci = 2 * (kb.ep_addr & 0x0F) + 1;

    // xHCI Interval field (microframe exponent), derived from bInterval + speed.
    let interval = xhci_interval(dev.speed, kb.interval);

    // Bytes each interrupt-IN Normal TRB requests: a boot keyboard always sends
    // an 8-byte report; a boot mouse sends its wMaxPacketSize (3–8 bytes).
    let report_len: u32 = if kb.proto == 1 {
        8
    } else {
        (kb.max_packet as u32).clamp(3, 8)
    };

    // Allocate the interrupt transfer ring (1 page, 256 TRBs).
    let int_ring = crate::memory::dma::alloc(1)?;
    // Install the Link TRB at the last slot so the ring wraps correctly.
    crate::usb::xhci::ring::init_link(int_ring.virt, int_ring.phys.as_u64(), true);

    // ── Build Input Context (csz-aware, mirror of address_device) ─────────────
    let csz = x.regs.capability.hccparams1.read_volatile().context_size();
    let int_phys = int_ring.phys.as_u64();

    if csz {
        // 64-byte contexts.
        let mut input = Input64Byte::new_64byte();
        {
            let ctrl = input.control_mut();
            ctrl.set_add_context_flag(0);          // A0 = Slot context
            ctrl.set_add_context_flag(dci as usize); // A(DCI) = endpoint context
        }
        {
            let dev_ctx = input.device_mut();
            {
                let slot = dev_ctx.slot_mut();
                // A0 is set, so the HC evaluates this Slot Context — it must be
                // fully valid, not just Context Entries. Re-supply the root-hub
                // port + speed (a stricter/real xHCI copies these into the output
                // slot context; zeroing them rejects or corrupts the slot).
                slot.set_context_entries(dci);
                slot.set_root_hub_port_number(dev.port);
                slot.set_speed(dev.speed);
            }
            {
                let ep = dev_ctx.endpoint_mut(dci as usize);
                ep.set_endpoint_type(EndpointType::InterruptIn);
                ep.set_max_packet_size(kb.max_packet);
                ep.set_interval(interval); // speed-aware, see xhci_interval()
                ep.set_tr_dequeue_pointer(int_phys); // 64-byte aligned phys, no DCS
                ep.set_dequeue_cycle_state();         // DCS = 1
                ep.set_error_count(3);
            }
        }
        let bytes = core::mem::size_of_val(&input);
        unsafe {
            core::ptr::copy_nonoverlapping(
                &input as *const _ as *const u8,
                dev.input_ctx.virt.as_mut_ptr::<u8>(),
                bytes,
            );
        }
    } else {
        // 32-byte contexts (QEMU default).
        let mut input = Input32Byte::new_32byte();
        {
            let ctrl = input.control_mut();
            ctrl.set_add_context_flag(0);
            ctrl.set_add_context_flag(dci as usize);
        }
        {
            let dev_ctx = input.device_mut();
            {
                let slot = dev_ctx.slot_mut();
                slot.set_context_entries(dci);
                slot.set_root_hub_port_number(dev.port);
                slot.set_speed(dev.speed);
            }
            {
                let ep = dev_ctx.endpoint_mut(dci as usize);
                ep.set_endpoint_type(EndpointType::InterruptIn);
                ep.set_max_packet_size(kb.max_packet);
                ep.set_interval(interval); // speed-aware, see xhci_interval()
                ep.set_tr_dequeue_pointer(int_phys);
                ep.set_dequeue_cycle_state();
                ep.set_error_count(3);
            }
        }
        let bytes = core::mem::size_of_val(&input);
        unsafe {
            core::ptr::copy_nonoverlapping(
                &input as *const _ as *const u8,
                dev.input_ctx.virt.as_mut_ptr::<u8>(),
                bytes,
            );
        }
    }

    // ── Issue Configure Endpoint command (type 12) ─────────────────────────────
    let in_phys = dev.input_ctx.phys.as_u64();
    let cfg_words = [
        (in_phys & 0xFFFF_FFF0) as u32,
        (in_phys >> 32) as u32,
        0u32,
        (dev.slot_id as u32) << 24,
    ];
    crate::usb::xhci::ring::enqueue_cmd(x, cfg_words, 12);
    let ev = match crate::usb::xhci::ring::wait_cmd(x) {
        Some(e) => e,
        None => {
            crate::bwarn!("usb", "config_ep: timeout slot={}", dev.slot_id);
            return None;
        }
    };
    let code = crate::usb::xhci::ring::completion_code(&ev);
    if code != 1 {
        crate::bwarn!("usb", "config_ep FAIL code={} slot={}", code, dev.slot_id);
        return None;
    }
    crate::binfo!("usb", "config_ep ok slot={} dci={}", dev.slot_id, dci);

    // ── SET_PROTOCOL (boot = 0): class-specific request to the HID interface ──
    // bmRequestType = 0x21 (Host→Device, Class, Interface)
    // bRequest = 0x0B (SET_PROTOCOL)
    // wValue = 0 (boot protocol)
    // wIndex = interface number
    // wLength = 0 (no data)
    let ok = crate::usb::control::control_out(
        x,
        dev,
        crate::usb::control::Setup {
            req_type: 0x21,
            request:  0x0B,
            value:    0,
            index:    kb.iface as u16,
            length:   0,
        },
    );
    if !ok {
        // Some devices STALL SET_PROTOCOL; log and continue — not fatal.
        crate::bwarn!("usb", "set_protocol STALL/fail — continuing");
    } else {
        crate::binfo!("usb", "set_protocol boot ok");
    }

    // ── Allocate report buffer (one page; report is report_len bytes) ─────────
    let report = crate::memory::dma::alloc(1)?;

    // ── Queue first Normal TRB onto the interrupt ring ─────────────────────────
    // Normal TRB (type 1):
    //   word0 = data buffer phys lo
    //   word1 = data buffer phys hi
    //   word2 = transfer length (report_len bytes)
    //   word3 = IOC(bit5) | type(bits10..15)=1
    // enqueue_xfer will bake the cycle bit into word3 bit0.
    let rp = report.phys.as_u64();
    let mut int_enqueue: usize = 0;
    let mut int_cycle: bool = true;
    crate::usb::xhci::ring::enqueue_xfer(
        &int_ring,
        &mut int_enqueue,
        &mut int_cycle,
        [
            (rp & 0xFFFF_FFFF) as u32,
            (rp >> 32) as u32,
            report_len,
            (1 << 10) | (1 << 5), // type=1 (Normal) | IOC
        ],
    );

    // ── Ring the endpoint doorbell (slot doorbell, target = DCI) ──────────────
    x.regs.doorbell.update_volatile_at(dev.slot_id as usize, |d| {
        d.set_doorbell_target(dci);
    });

    crate::binfo!(
        "usb", "hid boot {} ready slot={} dci={}",
        if kb.proto == 2 { "mouse" } else { "keyboard" }, dev.slot_id, dci
    );

    Some(HidState {
        slot_id: dev.slot_id,
        dci,
        int_ring,
        int_enqueue,
        int_cycle,
        report,
        report_len,
        prev: [0u8; 8],
    })
}

/// Process one completed HID report: edge-detect newly pressed keys, inject
/// their bytes into PTY 0, then re-queue a Normal TRB to receive the next report.
pub fn on_report(x: &mut Xhci, st: &mut HidState) {
    // Read the 8-byte report.
    let mut rep = [0u8; 8];
    let p = st.report.virt.as_ptr::<u8>();
    for i in 0..8 {
        rep[i] = unsafe { core::ptr::read_volatile(p.add(i)) };
    }
    let mods = rep[0];
    let prev_mods = st.prev[0];
    let shift = mods & 0x22 != 0;
    let ctrl  = mods & 0x11 != 0;

    // GUI key events (mirror the PS/2 driver, which feeds both PTY and the GUI):
    // emit modifier press/release edges as Set 1 scancodes so the egui desktop
    // tracks Shift/Ctrl/Alt state.
    for bit in 0u8..8 {
        let mask = 1u8 << bit;
        if (mods & mask) != (prev_mods & mask) {
            if let Some(sc) = crate::usb::usage::modifier_scancode(bit) {
                crate::gfx::push_key(sc, mods & mask != 0);
            }
        }
    }
    // Key releases: usages in the previous report no longer present (GUI only).
    for i in 2..8 {
        let code = st.prev[i];
        if code == 0 || code == 0x01 { continue; }
        if !rep[2..8].contains(&code) {
            if let Some(sc) = crate::usb::usage::usage_to_scancode(code) {
                crate::gfx::push_key(sc, false);
            }
        }
    }
    // Key presses: usages newly present. Feed the PTY (shell) a terminal byte AND
    // the GUI a Set 1 scancode-down event.
    for i in 2..8 {
        let code = rep[i];
        if code == 0 || code == 0x01 { continue; }
        if !st.prev[2..8].contains(&code) {
            if let Some(b) = crate::usb::usage::usage_to_byte(code, shift, ctrl) {
                crate::pty::master_input_push(0, b);
            }
            if let Some(sc) = crate::usb::usage::usage_to_scancode(code) {
                crate::gfx::push_key(sc, true);
            }
        }
    }
    st.prev = rep;
    // Re-queue a Normal TRB for the next report.
    let phys = st.report.phys.as_u64();
    let normal = [
        (phys & 0xFFFF_FFFF) as u32,
        (phys >> 32) as u32,
        8u32,
        (1 << 10) | (1 << 5), // type=1 (Normal) | IOC
    ];
    crate::usb::xhci::ring::enqueue_xfer(
        &st.int_ring,
        &mut st.int_enqueue,
        &mut st.int_cycle,
        normal,
    );
    x.regs.doorbell.update_volatile_at(st.slot_id as usize, |d| {
        d.set_doorbell_target(st.dci);
    });
}

/// Process one completed HID boot-mouse report: decode it into a `MouseEvent`,
/// inject it into the shared mouse queue (so the GUI cursor moves), then re-queue
/// a Normal TRB to receive the next report.
pub fn on_report_mouse(x: &mut Xhci, st: &mut HidState) {
    let len = st.report_len as usize;
    let mut rep = [0u8; 8];
    let p = st.report.virt.as_ptr::<u8>();
    for i in 0..len.min(8) {
        rep[i] = unsafe { core::ptr::read_volatile(p.add(i)) };
    }
    crate::mouse::inject(crate::usb::mouse::decode_boot_mouse(&rep[..len.min(8)]));

    // Re-queue a Normal TRB for the next report.
    let phys = st.report.phys.as_u64();
    let normal = [
        (phys & 0xFFFF_FFFF) as u32,
        (phys >> 32) as u32,
        st.report_len,
        (1 << 10) | (1 << 5), // type=1 (Normal) | IOC
    ];
    crate::usb::xhci::ring::enqueue_xfer(
        &st.int_ring,
        &mut st.int_enqueue,
        &mut st.int_cycle,
        normal,
    );
    x.regs.doorbell.update_volatile_at(st.slot_id as usize, |d| {
        d.set_doorbell_target(st.dci);
    });
}
