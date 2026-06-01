//! USB HID boot keyboard.

use crate::usb::xhci::Xhci;
use crate::usb::device::UsbDevice;
use crate::memory::dma::DmaRegion;

/// A detected HID boot keyboard's interrupt-IN endpoint.
#[derive(Clone, Copy)]
pub struct HidKeyboard {
    pub iface:      u8,   // bInterfaceNumber
    pub ep_addr:    u8,   // bEndpointAddress (bit7=IN, low4=EP number)
    pub max_packet: u16,  // wMaxPacketSize
    pub interval:   u8,   // bInterval
}

/// Running state for a configured HID keyboard: its interrupt transfer ring +
/// report buffer + last report (for edge detection in Task 9).
pub struct HidState {
    pub slot_id:      u8,
    pub dci:          u8,
    pub int_ring:     DmaRegion,
    pub int_enqueue:  usize,
    pub int_cycle:    bool,
    pub report:       DmaRegion,
    pub prev:         [u8; 8],
}

/// Configure the keyboard's interrupt-IN endpoint, set boot protocol, and
/// queue the first Normal TRB to receive a report. Returns the running state.
pub fn configure_endpoint(
    x: &mut Xhci,
    dev: &mut UsbDevice,
    kb: &HidKeyboard,
) -> Option<HidState> {
    use ::xhci::context::{
        Input32Byte, Input64Byte,
        InputHandler,
        EndpointType,
    };

    // DCI for interrupt-IN ep: 2 * (ep_number) + 1 (IN direction).
    // ep_addr 0x81 → ep_number = 0x81 & 0x0F = 1 → DCI = 3.
    let dci = 2 * (kb.ep_addr & 0x0F) + 1;

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
                // Highest valid DCI must be updated to include the new endpoint.
                slot.set_context_entries(dci);
            }
            {
                let ep = dev_ctx.endpoint_mut(dci as usize);
                ep.set_endpoint_type(EndpointType::InterruptIn);
                ep.set_max_packet_size(kb.max_packet);
                // xHCI interval field: bInterval from descriptor used directly
                // (QEMU high-speed: bInterval=7 is already the xHCI exponent).
                ep.set_interval(kb.interval);
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
            }
            {
                let ep = dev_ctx.endpoint_mut(dci as usize);
                ep.set_endpoint_type(EndpointType::InterruptIn);
                ep.set_max_packet_size(kb.max_packet);
                ep.set_interval(kb.interval);
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

    // ── Allocate report buffer (8 bytes for boot keyboard report) ─────────────
    let report = crate::memory::dma::alloc(1)?;

    // ── Queue first Normal TRB onto the interrupt ring ─────────────────────────
    // Normal TRB (type 1):
    //   word0 = data buffer phys lo
    //   word1 = data buffer phys hi
    //   word2 = transfer length (8 bytes)
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
            8u32,
            (1 << 10) | (1 << 5), // type=1 (Normal) | IOC
        ],
    );

    // ── Ring the endpoint doorbell (slot doorbell, target = DCI) ──────────────
    x.regs.doorbell.update_volatile_at(dev.slot_id as usize, |d| {
        d.set_doorbell_target(dci);
    });

    crate::binfo!("usb", "keyboard ready slot={} dci={}", dev.slot_id, dci);

    Some(HidState {
        slot_id: dev.slot_id,
        dci,
        int_ring,
        int_enqueue,
        int_cycle,
        report,
        prev: [0u8; 8],
    })
}
