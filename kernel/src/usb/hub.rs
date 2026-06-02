//! USB hub class driver: read the hub descriptor, configure the hub in xHCI
//! (slot-context hub fields + status-change interrupt endpoint), power its
//! ports, and enumerate devices behind it (recursively, with route string + TT).
//!
//! Single-TT only: we program `tt_think_time` but never `multi_tt` / SET_INTERFACE.
use crate::memory::dma::DmaRegion;
use crate::usb::control;
use crate::usb::device::{self, Location, UsbDevice};
use crate::usb::encoding;
use crate::usb::registry::{self, UsbAction};
use crate::usb::xhci::{ring, Xhci};

// Hub class port features (USB 2.0 §11.24.2).
const PORT_RESET: u16 = 4;
const PORT_POWER: u16 = 8;
const C_PORT_CONNECTION: u16 = 16;
const C_PORT_RESET: u16 = 20;

/// Running state for a configured hub.
pub struct HubState {
    pub dci: u8,
    pub nbr_ports: u8,
    pub int_ring: DmaRegion,
    pub int_enqueue: usize,
    pub int_cycle: bool,
    pub change_buf: DmaRegion,
}

/// Length (bytes) of the hub status-change bitmap: 1 bit per port + bit 0 (hub),
/// rounded up to bytes, at least 1.
fn change_len(nbr_ports: u8) -> u32 {
    (((nbr_ports as usize) + 1 + 7) / 8).max(1) as u32
}

/// Queue a Normal TRB on the hub's interrupt ring to receive the next status
/// change bitmap into `change_buf`, then ring the endpoint doorbell.
fn queue_status_trb(x: &mut Xhci, slot: u8, st: &mut HubState) {
    let phys = st.change_buf.phys.as_u64();
    let len = change_len(st.nbr_ports);
    let normal = [
        (phys & 0xFFFF_FFFF) as u32,
        (phys >> 32) as u32,
        len,
        (1 << 10) | (1 << 5), // type=1 (Normal) | IOC
    ];
    ring::enqueue_xfer(&st.int_ring, &mut st.int_enqueue, &mut st.int_cycle, normal);
    x.regs.doorbell.update_volatile_at(slot as usize, |d| {
        d.set_doorbell_target(st.dci);
    });
}

/// Walk the config descriptor for the hub: find bConfigurationValue + the hub
/// interface (bInterfaceClass==9) and its interrupt-IN endpoint. Returns
/// (cfg_val, ep_addr, max_packet, interval) or None.
fn read_hub_interface(x: &mut Xhci, dev: &mut UsbDevice) -> Option<(u8, u8, u16, u8)> {
    let buf = crate::memory::dma::alloc(1)?;
    let rd = |o: usize| unsafe { core::ptr::read_volatile(buf.virt.as_ptr::<u8>().add(o)) };

    // 9-byte config header → wTotalLength + bConfigurationValue.
    let s9 = control::Setup { req_type: 0x80, request: 6, value: 0x0200, index: 0, length: 9 };
    if control::control_in(x, dev, s9, &buf)? < 9 {
        crate::bwarn!("usb", "hub config header: short read");
        return None;
    }
    let total = ((rd(2) as u16) | ((rd(3) as u16) << 8)).min(4096);
    let cfg_val = rd(5);

    // Full config block.
    let s_all = control::Setup { req_type: 0x80, request: 6, value: 0x0200, index: 0, length: total };
    let n = control::control_in(x, dev, s_all, &buf)?;
    let n = (n.min(total)) as usize;

    // Walk descriptors: interface(4) sets current class; endpoint(5) under a
    // hub interface (class 9), IN + Interrupt, is the status-change endpoint.
    let mut pos: usize = 0;
    let mut in_hub_iface = false;
    let mut found: Option<(u8, u16, u8)> = None; // (ep_addr, mps, interval)
    while pos + 2 <= n {
        let blen = rd(pos) as usize;
        if blen == 0 || pos + blen > n {
            break;
        }
        match rd(pos + 1) {
            4 if pos + 9 <= n => {
                in_hub_iface = rd(pos + 5) == 9; // bInterfaceClass == Hub
            }
            5 if pos + 7 <= n => {
                let addr = rd(pos + 2);
                let attr = rd(pos + 3);
                if in_hub_iface && (addr & 0x80) != 0 && (attr & 0x03) == 3 && found.is_none() {
                    let mps = (rd(pos + 4) as u16) | ((rd(pos + 5) as u16) << 8);
                    found = Some((addr, mps, rd(pos + 6)));
                }
            }
            _ => {}
        }
        pos += blen;
    }

    let (ep_addr, mps, interval) = match found {
        Some(f) => f,
        None => {
            crate::bwarn!("usb", "hub: no interrupt-IN endpoint found");
            return None;
        }
    };
    Some((cfg_val, ep_addr, mps, interval))
}

/// Build + issue the Configure Endpoint command (type 12) that adds the hub's
/// status-change interrupt-IN endpoint AND fills the slot-context hub fields
/// (Hub bit, Number of Ports, TT Think Time). Mirrors `hid::configure_endpoint`'s
/// csz-aware Input-context build, plus the slot fields a hub needs. Returns true
/// on Success.
fn configure_hub_endpoint(
    x: &mut Xhci,
    dev: &mut UsbDevice,
    loc: &Location,
    dci: u8,
    mps: u16,
    interval: u8,
    int_ring_phys: u64,
    nbr_ports: u8,
    tt_think_time: u8,
) -> bool {
    use ::xhci::context::{EndpointType, Input32Byte, Input64Byte, InputHandler};

    // Slot-context builder shared by both context sizes. A0 is set, so the HC
    // evaluates this Slot Context fully — re-supply root-hub port + speed + route
    // + (for FS/LS-behind-HS) parent TT, then set the hub-specific fields.
    macro_rules! build_slot {
        ($slot:expr) => {{
            let slot = $slot;
            slot.set_context_entries(dci);
            slot.set_root_hub_port_number(loc.root_port);
            slot.set_route_string(loc.route);
            slot.set_speed(loc.speed);
            if loc.tt {
                slot.set_parent_hub_slot_id(loc.parent_slot);
                slot.set_parent_port_number(loc.parent_port);
            }
            // Hub-specific slot fields (single-TT: never set_multi_tt).
            slot.set_hub();
            slot.set_number_of_ports(nbr_ports);
            slot.set_tt_think_time(tt_think_time);
        }};
    }
    macro_rules! build_ep {
        ($dev_ctx:expr) => {{
            let ep = $dev_ctx.endpoint_mut(dci as usize);
            ep.set_endpoint_type(EndpointType::InterruptIn);
            ep.set_max_packet_size(mps);
            ep.set_interval(interval);
            ep.set_tr_dequeue_pointer(int_ring_phys);
            ep.set_dequeue_cycle_state();
            ep.set_error_count(3);
        }};
    }

    let csz = x.regs.capability.hccparams1.read_volatile().context_size();
    if csz {
        let mut input = Input64Byte::new_64byte();
        {
            let ctrl = input.control_mut();
            ctrl.set_add_context_flag(0); // A0 = slot context
            ctrl.set_add_context_flag(dci as usize); // A(dci) = endpoint context
        }
        {
            let dev_ctx = input.device_mut();
            build_slot!(dev_ctx.slot_mut());
            build_ep!(dev_ctx);
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
        let mut input = Input32Byte::new_32byte();
        {
            let ctrl = input.control_mut();
            ctrl.set_add_context_flag(0);
            ctrl.set_add_context_flag(dci as usize);
        }
        {
            let dev_ctx = input.device_mut();
            build_slot!(dev_ctx.slot_mut());
            build_ep!(dev_ctx);
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

    // Issue Configure Endpoint (type 12).
    let in_phys = dev.input_ctx.phys.as_u64();
    let cfg_words = [
        (in_phys & 0xFFFF_FFF0) as u32,
        (in_phys >> 32) as u32,
        0u32,
        (dev.slot_id as u32) << 24,
    ];
    ring::enqueue_cmd(x, cfg_words, 12);
    let ev = match ring::wait_cmd(x) {
        Some(e) => e,
        None => {
            crate::bwarn!("usb", "hub config_ep: timeout slot={}", dev.slot_id);
            return false;
        }
    };
    let code = ring::completion_code(&ev);
    if code != 1 {
        crate::bwarn!("usb", "hub config_ep FAIL code={} slot={}", code, dev.slot_id);
        return false;
    }
    crate::binfo!("usb", "hub config_ep ok slot={} dci={}", dev.slot_id, dci);
    true
}

/// Configure a hub and scan its ports.
///
/// 1. Read config descriptor → cfg_val + hub interrupt-IN endpoint.
/// 2. SET_CONFIGURATION.
/// 3. GET hub descriptor → nbr_ports / tt_think_time / pwr_on_2_pwr_good.
/// 4. Configure Endpoint (status-change EP + hub slot fields).
/// 5. Power all ports, wait power-good.
/// 6. Queue the first status-change Normal TRB.
/// 7. Initial scan: queue a HubPortChanged action per connected port.
pub fn setup(x: &mut Xhci, slot: u8, dev: &mut UsbDevice, loc: &Location) -> Option<HubState> {
    // 1. Config descriptor → cfg_val + interrupt-IN endpoint.
    let (cfg_val, ep_addr, mps, interval) = read_hub_interface(x, dev)?;
    crate::binfo!(
        "usb", "hub iface ep=0x{:02x} mps={} interval={}", ep_addr, mps, interval
    );

    // 2. SET_CONFIGURATION.
    if !control::control_out(
        x,
        dev,
        control::Setup { req_type: 0x00, request: 9, value: cfg_val as u16, index: 0, length: 0 },
    ) {
        crate::bwarn!("usb", "hub set_config failed");
        return None;
    }

    // 3. Hub descriptor.
    let hbuf = crate::memory::dma::alloc(1)?;
    let got = control::get_hub_descriptor(x, dev, &hbuf)?;
    let d = {
        let n = (got as usize).min(71);
        let mut tmp = [0u8; 71];
        let p = hbuf.virt.as_ptr::<u8>();
        for i in 0..n {
            tmp[i] = unsafe { core::ptr::read_volatile(p.add(i)) };
        }
        tmp
    };
    let hd = match encoding::decode_hub_desc(&d) {
        Some(h) => h,
        None => {
            crate::bwarn!("usb", "hub: bad hub descriptor");
            return None;
        }
    };
    crate::memory::dma::dealloc(hbuf);
    let nbr_ports = hd.nbr_ports;

    // 4. Allocate the status-change interrupt ring + Configure Endpoint.
    let dci = 2 * (ep_addr & 0x0F) + 1;
    let int_ring = crate::memory::dma::alloc(1)?;
    ring::init_link(int_ring.virt, int_ring.phys.as_u64(), true);
    if !configure_hub_endpoint(
        x,
        dev,
        loc,
        dci,
        mps,
        interval,
        int_ring.phys.as_u64(),
        nbr_ports,
        hd.tt_think_time,
    ) {
        crate::memory::dma::dealloc(int_ring);
        return None;
    }

    // 5. Power all ports, then wait power-good (bounded).
    for port in 1..=nbr_ports {
        if !control::set_port_feature(x, dev, port, PORT_POWER) {
            crate::bwarn!("usb", "hub: PORT_POWER failed port={}", port);
        }
    }
    let pg_wait = (hd.pwr_on_2_pwr_good_ms as u64).min(500).max(10);
    let start = crate::boot::clock::elapsed_ms();
    while crate::boot::clock::elapsed_ms() - start < pg_wait {
        core::hint::spin_loop();
    }

    // 6. Allocate change buffer + queue the first status-change Normal TRB.
    let change_buf = crate::memory::dma::alloc(1)?;
    let mut st = HubState {
        dci,
        nbr_ports,
        int_ring,
        int_enqueue: 0,
        int_cycle: true,
        change_buf,
    };
    queue_status_trb(x, slot, &mut st);

    // 7. Initial scan: a HubPortChanged per already-connected port so the
    //    worklist resets + enumerates anything plugged in at boot.
    let pbuf = crate::memory::dma::alloc(1)?;
    for port in 1..=nbr_ports {
        if let Some((status, _change)) = control::get_port_status(x, dev, port, &pbuf) {
            if encoding::decode_port_status(status, 0).connected {
                registry::push_action(UsbAction::HubPortChanged { hub_slot: slot, port });
            }
        }
    }
    crate::memory::dma::dealloc(pbuf);

    // Marker scraped by tests/usb-hub-test.sh. The logger renders this as
    // `INFO usb hub slot=N ports=M` (module field "usb hub" exceeds the 4-char
    // pad, so it prints verbatim with a single space before `hub`).
    crate::binfo!("usb hub", "slot={} ports={}", slot, nbr_ports);
    Some(st)
}

/// Hub interrupt completed: read the change bitmap, queue a port action per set
/// bit, then re-queue the Normal TRB. MUST NOT lock SLOTS / drain events — it
/// runs from `dispatch_transfer` with SLOTS already held.
pub fn on_status(x: &mut Xhci, slot: u8, st: &mut HubState) {
    let p = st.change_buf.virt.as_ptr::<u8>();
    let nb = st.nbr_ports;
    for port in 1..=nb {
        let byte = unsafe { core::ptr::read_volatile(p.add((port / 8) as usize)) };
        if byte & (1 << (port % 8)) != 0 {
            registry::push_action(UsbAction::HubPortChanged { hub_slot: slot, port });
        }
    }
    // Re-arm: queue the next status-change Normal TRB + ring the doorbell.
    queue_status_trb(x, slot, st);
}

/// What to do after inspecting a hub port, decided while the hub entry is
/// borrowed; acted on (enumerate/teardown) only after the borrow is released.
enum Outcome {
    Connect { child: Location },
    Disconnect,
    None,
}

/// Worklist action: a hub port changed. GET_STATUS the port; on connect reset +
/// enumerate the child, on disconnect tear the child down.
///
/// # Lock / borrow structure
/// We need the hub's `&mut UsbDevice` (for control transfers to the hub) but must
/// NOT call `enumerate`/`teardown` while holding SLOTS (they lock SLOTS and would
/// deadlock / re-enter). So:
///   1. Inside a single `with_slot(hub_slot, ...)` closure — which borrows the
///      hub `SlotEntry` and captures `x` — run ALL the hub's control transfers
///      (get_port_status, set/clear feature, reset poll) and build the child
///      `Location` from the hub's topology fields. These control transfers do
///      not reach `dispatch_transfer` (every `event::wait_for` here matches on
///      Transfer-Event type 32 and returns before dispatching one), so they
///      never re-lock SLOTS — safe under the held lock. The closure returns an
///      `Outcome` (Connect{child} / Disconnect / None) by value.
///   2. AFTER `with_slot` returns (SLOTS released): act on the `Outcome` —
///      `enumerate(child)` (connect) or `find_child` + `teardown` (disconnect),
///      each of which locks SLOTS itself, never under a held lock.
pub fn handle_port(x: &mut Xhci, hub_slot: u8, port: u8) {
    let pbuf = match crate::memory::dma::alloc(1) {
        Some(b) => b,
        None => return,
    };

    // ── Phase 1: hub control transfers under the SLOTS lock (no enumerate). ──
    let already = registry::find_child(hub_slot, port);
    let outcome = registry::with_slot(hub_slot, |e| {
        let (route, tier, speed, root_port) = (e.route, e.tier, e.speed, e.root_port);
        let dev = &mut e.dev;

        let (status, _change) = match control::get_port_status(x, dev, port, &pbuf) {
            Some(v) => v,
            None => return Outcome::None,
        };
        let ps = encoding::decode_port_status(status, 0);

        if ps.connected && already.is_none() {
            // Reset the port, then poll until reset clears (bounded 100 ms).
            control::set_port_feature(x, dev, port, PORT_RESET);
            let start = crate::boot::clock::elapsed_ms();
            let mut child_speed = ps.speed;
            loop {
                if crate::boot::clock::elapsed_ms() - start >= 100 {
                    break;
                }
                if let Some((s2, c2)) = control::get_port_status(x, dev, port, &pbuf) {
                    let p2 = encoding::decode_port_status(s2, c2);
                    child_speed = p2.speed;
                    // Reset done = PORT_RESET cleared, or C_PORT_RESET (bit4) set.
                    if !p2.reset || (c2 & (1 << 4)) != 0 {
                        break;
                    }
                }
                core::hint::spin_loop();
            }
            control::clear_port_feature(x, dev, port, C_PORT_RESET);
            control::clear_port_feature(x, dev, port, C_PORT_CONNECTION);

            if tier + 1 >= encoding::MAX_TIER {
                crate::bwarn!("usb", "hub: tier limit, skip port={} (tier={})", port, tier);
                Outcome::None
            } else {
                Outcome::Connect {
                    child: Location {
                        root_port,
                        route: encoding::child_route(route, port, tier),
                        tier: tier + 1,
                        speed: child_speed,
                        parent_slot: hub_slot,
                        parent_port: port,
                        tt: encoding::needs_tt(child_speed, speed),
                    },
                }
            }
        } else if !ps.connected {
            // Disconnect: ack the connection change so the hub stops asserting it.
            control::clear_port_feature(x, dev, port, C_PORT_CONNECTION);
            Outcome::Disconnect
        } else {
            // Connected but already enumerated (spurious/extra change bit): ack.
            control::clear_port_feature(x, dev, port, C_PORT_CONNECTION);
            Outcome::None
        }
    })
    .unwrap_or(Outcome::None); // hub entry gone
    crate::memory::dma::dealloc(pbuf);

    // ── Phase 2: SLOTS free — enumerate / teardown the child. ───────────────
    match outcome {
        Outcome::Connect { child } => {
            crate::binfo!(
                "usb", "hub port {} connect speed={} route=0x{:X} tt={}",
                port, child.speed, child.route, child.tt
            );
            let _ = device::enumerate(x, child);
        }
        Outcome::Disconnect => {
            if let Some(c) = registry::find_child(hub_slot, port) {
                crate::binfo!("usb", "hub port {} disconnect, teardown slot={}", port, c);
                registry::teardown(x, c);
            }
        }
        Outcome::None => {}
    }
}
