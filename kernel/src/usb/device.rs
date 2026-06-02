//! USB device enumeration: root-port scan/reset, slot allocation, addressing.
use crate::usb::xhci::Xhci;
use crate::usb::xhci::ring;
use crate::memory::dma::DmaRegion;

/// Where a device sits in the USB topology — everything `enumerate` needs to
/// build the slot context (root device: route=0, tier=0, parent_slot=0, tt=false;
/// devices behind a hub fill route/tier/parent_* and set tt for FS/LS-via-HS-hub).
#[derive(Clone, Copy)]
pub struct Location {
    pub root_port: u8,   // root-hub port this device's branch hangs off (1-based)
    pub route: u32,      // xHCI route string (0 for a root device)
    pub tier: u8,        // hub depth (0 = root-attached)
    pub speed: u8,       // PSI value (1=Full, 2=Low, 3=High, 4=Super)
    pub parent_slot: u8, // slot id of the parent hub (0 = root)
    pub parent_port: u8, // 1-based port on the parent hub (0 = root)
    pub tt: bool,        // needs Transaction Translator (FS/LS device on an HS hub)
}

/// Reset a single connected root port and read its operating speed. Returns
/// `Some(speed)` (PSI value) once the port is reset+enabled, `None` if nothing is
/// connected or the port never enabled. The worklist (`xhci::poll`) calls this
/// per port before `enumerate`.
///
/// # PORTSC RW1C note
/// `update_volatile_at` does a plain read-modify-write. PORTSC contains several
/// Read-Write-1-to-Clear (RW1C) status-change bits; to avoid accidentally
/// clearing them during the set_port_reset() write, we call `set_0_*` on every
/// RW1C change bit in the same closure so they stay 0 in the written value.
/// Only when we deliberately clear PRC do we call `clear_port_reset_change()`.
pub fn reset_root_port(x: &mut Xhci, port: u8) -> Option<u8> {
    let idx = (port - 1) as usize;

    // Check if a device is connected (CCS = Current Connect Status).
    let p = x.regs.port_register_set.read_volatile_at(idx);
    if !p.portsc.current_connect_status() {
        return None;
    }

    // Assert port reset (PR bit, RW1S). Preserve all RW1C change bits by
    // writing 0 to them so the read-modify-write does not accidentally clear
    // them (writing 1 to a RW1C bit clears it in hardware).
    x.regs.port_register_set.update_volatile_at(idx, |p| {
        p.portsc.set_port_reset();
        // Write 0 to all RW1C change bits to avoid clearing them.
        p.portsc.set_0_port_enabled_disabled();
        p.portsc.set_0_connect_status_change();
        p.portsc.set_0_port_enabled_disabled_change();
        p.portsc.set_0_warm_port_reset_change();
        p.portsc.set_0_over_current_change();
        p.portsc.set_0_port_reset_change();
        p.portsc.set_0_port_link_state_change();
        p.portsc.set_0_port_config_error_change();
    });

    // Wait (bounded 50 ms) for reset to complete — PRC (Port Reset Change) set.
    let start = crate::boot::clock::elapsed_ms();
    let mut reset_done = false;
    while crate::boot::clock::elapsed_ms() - start < 50 {
        let p = x.regs.port_register_set.read_volatile_at(idx);
        if p.portsc.port_reset_change() {
            reset_done = true;
            break;
        }
        core::hint::spin_loop();
    }

    // Clear PRC (RW1C) and preserve all other RW1C change bits.
    x.regs.port_register_set.update_volatile_at(idx, |p| {
        p.portsc.clear_port_reset_change();   // write 1 → hardware clears PRC
        // Write 0 to all other RW1C change bits so we don't clear them.
        p.portsc.set_0_port_enabled_disabled();
        p.portsc.set_0_connect_status_change();
        p.portsc.set_0_port_enabled_disabled_change();
        p.portsc.set_0_warm_port_reset_change();
        p.portsc.set_0_over_current_change();
        p.portsc.set_0_port_link_state_change();
        p.portsc.set_0_port_config_error_change();
    });

    let p = x.regs.port_register_set.read_volatile_at(idx);
    let speed   = p.portsc.port_speed();
    let enabled = p.portsc.port_enabled_disabled();

    crate::binfo!(
        "usb",
        "port {} connected speed={} enabled={} reset_done={}",
        port, speed, enabled, reset_done
    );

    if enabled { Some(speed) } else { None }
}

/// An addressed USB device: slot allocated, EP0 transfer ring set up, Device Context
/// installed in DCBAA. Ready for descriptor fetch (Task 6+).
pub struct UsbDevice {
    pub slot_id:     u8,
    pub port:        u8,
    pub speed:       u8,
    pub max_packet0: u16,
    pub ep0_ring:    DmaRegion,
    pub input_ctx:   DmaRegion,
    pub dev_ctx:     DmaRegion,
    pub ep0_enqueue: usize,
    pub ep0_cycle:   bool,
}

/// Enumerate the device at `loc`: Enable Slot → build Input Context (root-hub
/// port + speed + route string + TT) → allocate EP0 ring + Device Context →
/// write DCBAA → Address Device → read the device descriptor → `configure` →
/// class-dispatch (keyboard / hub / other) → register the slot. Returns the
/// allocated slot id, or `None` on any failure (DMA leaks on the error path are
/// accepted for now — boot/hot-plug is rare; teardown frees registered slots).
pub fn enumerate(x: &mut Xhci, loc: Location) -> Option<u8> {
    // NB: only `InputHandler` is imported — `device_mut()`/`slot_mut()`/
    // `endpoint_mut()` return `&mut dyn {Device,Slot,Endpoint}Handler`, so those
    // traits' methods dispatch through the trait object without being in scope.
    use ::xhci::context::{Input32Byte, Input64Byte, InputHandler, EndpointType};

    // ── 1. Enable Slot (command type 9) ─────────────────────────────────────
    ring::enqueue_cmd(x, [0, 0, 0, 0], 9);
    let ev = match ring::wait_cmd(x) {
        Some(e) => e,
        None => { crate::bwarn!("usb", "enable slot: timeout"); return None; }
    };
    let code = ring::completion_code(&ev);
    if code != 1 {
        crate::bwarn!("usb", "enable slot FAIL code={}", code);
        return None;
    }
    let slot_id = ((ev[3] >> 24) & 0xFF) as u8;
    crate::binfo!("usb", "slot {} enabled", slot_id);

    // ── 2. MaxPacketSize0 by speed ───────────────────────────────────────────
    let max_packet0: u16 = crate::usb::encoding::max_packet0(loc.speed);

    // ── 3. Allocate DMA regions ──────────────────────────────────────────────
    let ep0_ring = match crate::memory::dma::alloc(1) {
        Some(r) => r,
        None => { crate::bwarn!("usb", "ep0 ring alloc failed"); return None; }
    };
    let dev_ctx = match crate::memory::dma::alloc(1) {
        Some(r) => r,
        None => { crate::bwarn!("usb", "dev ctx alloc failed"); return None; }
    };
    let input_ctx = match crate::memory::dma::alloc(1) {
        Some(r) => r,
        None => { crate::bwarn!("usb", "input ctx alloc failed"); return None; }
    };

    // ── 4. EP0 transfer ring: install Link TRB at index 255, DCS=1 ──────────
    ring::init_link(ep0_ring.virt, ep0_ring.phys.as_u64(), true);

    // ── 5. Write DCBAA[slot_id] = dev_ctx.phys ──────────────────────────────
    unsafe {
        x.dcbaa.virt.as_mut_ptr::<u64>()
            .add(slot_id as usize)
            .write_volatile(dev_ctx.phys.as_u64());
    }

    // ── 6. Build Input Context (csz-aware) and memcpy into input_ctx DMA ────
    let csz = x.regs.capability.hccparams1.read_volatile().context_size();
    // The xhci crate's set_tr_dequeue_pointer asserts 64-byte alignment and
    // stores the raw address (no DCS). DCS is set separately via
    // set_dequeue_cycle_state(). Our DMA pages are 4KiB-aligned, so fine.
    let ep0_phys = ep0_ring.phys.as_u64();

    // Slot-context builder shared by both context sizes: root-hub port + speed,
    // route string (0 for a root device), and — for a FS/LS device behind an HS
    // hub — the parent hub slot/port so the HC routes split transactions via TT.
    macro_rules! build_slot {
        ($slot:expr) => {{
            let slot = $slot;
            slot.set_context_entries(1);
            slot.set_root_hub_port_number(loc.root_port);
            slot.set_speed(loc.speed);
            slot.set_route_string(loc.route);
            if loc.tt {
                slot.set_parent_hub_slot_id(loc.parent_slot);
                slot.set_parent_port_number(loc.parent_port);
            }
        }};
    }

    if csz {
        // 64-byte contexts
        let mut input = Input64Byte::new_64byte();
        {
            let ctrl = input.control_mut();
            ctrl.set_add_context_flag(0); // A0 = slot context
            ctrl.set_add_context_flag(1); // A1 = EP0 context
        }
        {
            let dev = input.device_mut();
            build_slot!(dev.slot_mut());
            {
                let ep0 = dev.endpoint_mut(1); // DCI 1 = Control EP0
                ep0.set_endpoint_type(EndpointType::Control);
                ep0.set_max_packet_size(max_packet0);
                ep0.set_tr_dequeue_pointer(ep0_phys); // 64-byte aligned phys
                ep0.set_dequeue_cycle_state();         // DCS = 1
                ep0.set_error_count(3);
            }
        }
        let bytes = core::mem::size_of_val(&input);
        unsafe {
            core::ptr::copy_nonoverlapping(
                &input as *const _ as *const u8,
                input_ctx.virt.as_mut_ptr::<u8>(),
                bytes,
            );
        }
    } else {
        // 32-byte contexts (QEMU default)
        let mut input = Input32Byte::new_32byte();
        {
            let ctrl = input.control_mut();
            ctrl.set_add_context_flag(0); // A0 = slot context
            ctrl.set_add_context_flag(1); // A1 = EP0 context
        }
        {
            let dev = input.device_mut();
            build_slot!(dev.slot_mut());
            {
                let ep0 = dev.endpoint_mut(1); // DCI 1 = Control EP0
                ep0.set_endpoint_type(EndpointType::Control);
                ep0.set_max_packet_size(max_packet0);
                ep0.set_tr_dequeue_pointer(ep0_phys); // 64-byte aligned phys
                ep0.set_dequeue_cycle_state();         // DCS = 1
                ep0.set_error_count(3);
            }
        }
        let bytes = core::mem::size_of_val(&input);
        unsafe {
            core::ptr::copy_nonoverlapping(
                &input as *const _ as *const u8,
                input_ctx.virt.as_mut_ptr::<u8>(),
                bytes,
            );
        }
    }

    // ── 7. Address Device command (type 11) ──────────────────────────────────
    // word0 = input_ctx phys lo (16-byte aligned, low 4 bits = 0)
    // word1 = input_ctx phys hi
    // word2 = 0
    // word3 bits 24..=31 = slot_id (enqueue_cmd preserves these bits)
    let in_phys = input_ctx.phys.as_u64();
    let addr_words = [
        (in_phys & 0xFFFF_FFF0) as u32,
        (in_phys >> 32) as u32,
        0u32,
        (slot_id as u32) << 24,
    ];
    ring::enqueue_cmd(x, addr_words, 11);
    let ev2 = match ring::wait_cmd(x) {
        Some(e) => e,
        None => { crate::bwarn!("usb", "address device: timeout slot={}", slot_id); return None; }
    };
    let code2 = ring::completion_code(&ev2);
    if code2 != 1 {
        crate::bwarn!("usb", "address FAIL code={} slot={}", code2, slot_id);
        return None;
    }

    crate::binfo!(
        "usb", "slot {} addressed port={} speed={} mps0={} route=0x{:X}",
        slot_id, loc.root_port, loc.speed, max_packet0, loc.route
    );

    let mut dev = UsbDevice {
        slot_id,
        port: loc.root_port,
        speed: loc.speed,
        max_packet0,
        ep0_ring,
        input_ctx,
        dev_ctx,
        ep0_enqueue: 0,
        ep0_cycle: true,
    };

    // ── 8. Device descriptor (for bDeviceClass) + class dispatch ─────────────
    let dev_class = read_device_descriptor(x, &mut dev).unwrap_or(0);
    let kb = configure(x, &mut dev);

    let kind = if let Some(kb) = kb {
        // HID boot keyboard: configure its interrupt-IN endpoint + queue a report.
        let st = crate::usb::hid::configure_endpoint(x, &mut dev, &kb)?;
        crate::usb::registry::SlotKind::Keyboard(st)
    } else if dev_class == 0x09 {
        // USB hub (QEMU usb-hub reports class 9 on the device descriptor).
        let hs = crate::usb::hub::setup(x, slot_id, &mut dev, &loc)?;
        crate::usb::registry::SlotKind::Hub(hs)
    } else {
        crate::usb::registry::SlotKind::Other
    };

    // ── 9. Register the slot (lock held only for the insert) ─────────────────
    crate::usb::registry::insert(slot_id, crate::usb::registry::SlotEntry {
        kind,
        dev,
        root_port:   loc.root_port,
        parent_slot: loc.parent_slot,
        parent_port: loc.parent_port,
        route:       loc.route,
        tier:        loc.tier,
        speed:       loc.speed,
    });

    crate::binfo!("usb", "enumerated slot={} route=0x{:X}", slot_id, loc.route);
    Some(slot_id)
}

/// Read the Configuration Descriptor, walk interface/endpoint descriptors to
/// find the HID boot keyboard interrupt-IN endpoint, then issue SET_CONFIGURATION.
/// Returns the HidKeyboard descriptor on success.
pub fn configure(x: &mut Xhci, dev: &mut UsbDevice) -> Option<crate::usb::hid::HidKeyboard> {
    // NB: this scratch page is intentionally leaked (not dma::dealloc'd). It is a
    // single one-shot page used only during boot enumeration; freeing across the
    // `?` early-returns isn't worth the control-flow risk for one page.
    let buf = crate::memory::dma::alloc(1)?;

    // ── 1. Read 9-byte config header for wTotalLength + bConfigurationValue ──
    let s9 = crate::usb::control::Setup {
        req_type: 0x80,
        request:  6,
        value:    0x0200, // Config descriptor, index 0
        index:    0,
        length:   9,
    };
    if crate::usb::control::control_in(x, dev, s9, &buf)? < 9 {
        crate::bwarn!("usb", "config header: short read");
        return None;
    }
    let rd = |o: usize| unsafe { core::ptr::read_volatile(buf.virt.as_ptr::<u8>().add(o)) };
    let total = (rd(2) as u16) | ((rd(3) as u16) << 8);
    let cfg_val = rd(5);

    // ── 2. Read the full config block (capped at 4096) ────────────────────────
    let total = total.min(4096);
    let s_all = crate::usb::control::Setup {
        req_type: 0x80,
        request:  6,
        value:    0x0200,
        index:    0,
        length:   total,
    };
    let n = crate::usb::control::control_in(x, dev, s_all, &buf)?;
    let n = (n.min(total)) as usize;

    // ── 3. Walk descriptors ───────────────────────────────────────────────────
    let mut pos: usize = 0;
    // (iface_num, class, subclass, protocol)
    let mut cur_iface: Option<(u8, u8, u8, u8)> = None;
    let mut found: Option<crate::usb::hid::HidKeyboard> = None;

    while pos + 2 <= n {
        let blen = rd(pos) as usize;
        if blen == 0 || pos + blen > n {
            break;
        }
        let dtype = rd(pos + 1);
        match dtype {
            4 => {
                // Interface descriptor
                if pos + 9 <= n {
                    cur_iface = Some((rd(pos + 2), rd(pos + 5), rd(pos + 6), rd(pos + 7)));
                }
            }
            5 => {
                // Endpoint descriptor
                if pos + 7 <= n {
                    if let Some((iface, cls, sub, proto)) = cur_iface {
                        let addr = rd(pos + 2);
                        let attr = rd(pos + 3);
                        let is_in  = (addr & 0x80) != 0;
                        let is_int = (attr & 0x03) == 3;
                        if cls == 3 && sub == 1 && proto == 1 && is_in && is_int
                            && found.is_none()
                        {
                            let mps = (rd(pos + 4) as u16) | ((rd(pos + 5) as u16) << 8);
                            found = Some(crate::usb::hid::HidKeyboard {
                                iface,
                                ep_addr:    addr,
                                max_packet: mps,
                                interval:   rd(pos + 6),
                            });
                        }
                    }
                }
            }
            _ => {}
        }
        pos += blen;
    }

    match found {
        Some(kb) => {
            crate::binfo!(
                "usb",
                "HID kbd iface={} ep=0x{:02x} mps={} interval={}",
                kb.iface, kb.ep_addr, kb.max_packet, kb.interval
            );
        }
        None => {
            crate::bwarn!("usb", "no HID boot keyboard found");
            return None;
        }
    }

    // ── 4. SET_CONFIGURATION ─────────────────────────────────────────────────
    let ok = crate::usb::control::control_out(
        x,
        dev,
        crate::usb::control::Setup {
            req_type: 0x00,
            request:  9,
            value:    cfg_val as u16,
            index:    0,
            length:   0,
        },
    );
    if ok {
        crate::binfo!("usb", "slot {} configured", dev.slot_id);
    } else {
        crate::bwarn!("usb", "set_config failed");
        return None;
    }

    found
}

/// Read the 18-byte USB Device Descriptor from the addressed device and log
/// VID, PID, class, max_packet_size0, and number of configurations. Returns the
/// bDeviceClass byte (offset 4) on success — used for hub vs. other dispatch.
pub fn read_device_descriptor(x: &mut Xhci, dev: &mut UsbDevice) -> Option<u8> {
    let buf = match crate::memory::dma::alloc(1) {
        Some(b) => b,
        None => {
            crate::bwarn!("usb", "device descriptor: DMA alloc failed");
            return None;
        }
    };

    let setup = crate::usb::control::Setup {
        req_type: 0x80,        // Device→Host, Standard, Device
        request:  6,           // GET_DESCRIPTOR
        value:    0x0100,      // Descriptor Type=Device (0x01), Index=0
        index:    0,
        length:   18,
    };

    match crate::usb::control::control_in(x, dev, setup, &buf) {
        Some(n) if n >= 18 => {
            let d = buf.virt.as_ptr::<u8>();
            // SAFETY: DMA buffer is mapped, readable, and at least 18 bytes long.
            let rd   = |o: usize| unsafe { core::ptr::read_volatile(d.add(o)) };
            let rd16 = |o: usize| (rd(o) as u16) | ((rd(o + 1) as u16) << 8);
            let vid      = rd16(8);
            let pid      = rd16(10);
            let class    = rd(4);
            let mps0     = rd(7);
            let num_cfg  = rd(17);
            crate::binfo!(
                "usb",
                "dev {:04x}:{:04x} class={} maxpkt0={} numcfg={}",
                vid, pid, class, mps0, num_cfg
            );
            Some(class)
        }
        Some(n) => {
            crate::bwarn!("usb", "device descriptor short: got {} bytes", n);
            None
        }
        None => {
            crate::bwarn!("usb", "device descriptor read failed");
            None
        }
    }
}
