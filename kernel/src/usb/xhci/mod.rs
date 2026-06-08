//! xHCI host controller driver — Task 3: TRB ring abstraction + No-Op round-trip.
//!
//! Sequence (xHCI 1.2 §4.2): find/enable PCI device, wait CNR, halt, reset,
//! set MaxSlotsEn, allocate DCBAA + scratchpad + command ring + event ring,
//! program registers, run, issue No-Op command, verify Command Completion event.
pub mod regs;
pub mod ring;
pub mod event;
pub mod bulk;

use crate::memory::dma::{self, DmaRegion};
use crate::pci;
use regs::HhdmMapper;
use alloc::vec::Vec;

/// xHCI controller state — holds DMA regions and ring bookkeeping.
pub struct Xhci {
    pub idx:          u8, // index of this controller in `usb::CTRLS`
    pub regs:         ::xhci::Registers<HhdmMapper>,
    pub max_slots:    u8,
    pub max_ports:    u8,
    pub dcbaa:        DmaRegion,
    pub cmd_ring:     DmaRegion,
    pub event_ring:   DmaRegion,
    pub erst:         DmaRegion,
    pub scratchpad:   Option<DmaRegion>,
    pub scratch_bufs: Vec<DmaRegion>,
    pub cmd_cycle:    bool,
    pub cmd_enqueue:  usize,
    pub event_cycle:  bool,
    pub event_dequeue: usize,
}

/// Spin until `predicate()` returns true, or until `timeout_ms` milliseconds
/// have elapsed. Returns `true` if the predicate became true in time.
fn wait_ms<F: Fn() -> bool>(predicate: F, timeout_ms: u64) -> bool {
    let deadline = crate::boot::clock::elapsed_ms() + timeout_ms;
    loop {
        if predicate() { return true; }
        if crate::boot::clock::elapsed_ms() >= deadline { return false; }
        core::hint::spin_loop();
    }
}

/// Take xHCI ownership from the BIOS and silence its legacy SMIs (real hardware).
///
/// Walks the xHCI Extended Capability list (raw MMIO) for the USB Legacy Support
/// capability (id 1). If the firmware owns the controller, set the OS-owned
/// semaphore and wait (bounded) for the BIOS to release; then write USBLEGCTLSTS
/// = 0xE000_0000 — all SMI enables cleared, all three RW1C SMI status bits
/// written-1-to-clear — so the firmware can no longer raise legacy SMIs and stall
/// the machine. `bar_virt` is the HHDM virtual address of xHCI BAR0. No-op on
/// controllers without extended capabilities (e.g. QEMU's qemu-xhci).
fn bios_handoff(bar_virt: u64) {
    use core::ptr::{read_volatile, write_volatile};
    // HCCPARAMS1 @ capability offset 0x10; xECP = bits 31:16, in dwords.
    let hcc1 = unsafe { read_volatile((bar_virt + 0x10) as *const u32) };
    let xecp = ((hcc1 >> 16) & 0xFFFF) as u64;
    if xecp == 0 { return; }
    let mut cap = bar_virt + xecp * 4;
    // Bounded walk (guard against a malformed/looping list on bad hardware).
    for _ in 0..64 {
        let dw = unsafe { read_volatile(cap as *const u32) };
        let id = dw & 0xFF;
        let next = ((dw >> 8) & 0xFF) as u64; // dwords to next cap
        if id == 1 {
            // USBLEGSUP @ cap, USBLEGCTLSTS @ cap+4.
            // Request OS ownership (bit 24).
            unsafe { write_volatile(cap as *mut u32, dw | (1 << 24)); }
            // Wait (bounded 100 ms) for the BIOS-owned semaphore (bit 16) to clear.
            let deadline = crate::boot::clock::elapsed_ms() + 100;
            while crate::boot::clock::elapsed_ms() < deadline {
                if unsafe { read_volatile(cap as *const u32) } & (1 << 16) == 0 { break; }
                core::hint::spin_loop();
            }
            // Force OS-owned + BIOS-not-owned regardless of timeout (best effort).
            let v = unsafe { read_volatile(cap as *const u32) };
            unsafe { write_volatile(cap as *mut u32, (v | (1 << 24)) & !(1 << 16)); }
            // Disable ALL legacy SMIs + clear the RW1C SMI status bits (29..31).
            unsafe { write_volatile((cap + 4) as *mut u32, 0xE000_0000); }
            crate::binfo!("usb", "xhci BIOS->OS handoff done");
            return;
        }
        if next == 0 { break; }
        cap += next * 4;
    }
}

/// Bring up ONE xHCI controller (`idx` = its slot in `usb::CTRLS`). Non-fatal:
/// logs a warning and returns `None` on any error so a broken controller does
/// not hang the system or block the others.
fn bringup(dev: pci::PciDevice, idx: u8) -> Option<Xhci> {
    // ── 1. PCI: enable, map BAR0 ────────────────────────────────────────────
    dev.enable_mmio();
    dev.enable_bus_master();
    let (base, size) = match dev.bar(0) {
        Some(pci::Bar::Memory64 { address, size, .. }) => (address, size as usize),
        Some(pci::Bar::Memory32 { address, size, .. }) => (address as u64, size as usize),
        other => { crate::bwarn!("usb", "xhci bar0 unexpected: {:?}", other); return None; }
    };

    // SAFETY: `base` is the xHCI BAR0 physical address; HhdmMapper maps each
    // register block into the HHDM virtual window.
    let mut regs = unsafe {
        ::xhci::Registers::new(base as usize, HhdmMapper)
    };

    // ── 1b. BIOS→OS handoff + disable legacy SMIs (real hardware) ─────────────
    // On real HW the firmware owns the xHCI (USB legacy keyboard/boot support);
    // resetting/running it while the BIOS still owns it — with its SMI handler
    // active — makes SMM fight us for the controller and the machine FREEZES
    // (SMM preempts the OS, so our bounded waits can't save us). QEMU exposes no
    // extended capabilities / no SMM, so this is a no-op there. Map the whole
    // BAR first so the extended-capability list (which can sit outside the
    // register blocks the crate mapped) is reachable.
    if let Ok(bar_virt) = crate::memory::mapper::map_io_range(x86_64::PhysAddr::new(base), size) {
        bios_handoff(bar_virt.as_u64());
    }

    let hcs1     = regs.capability.hcsparams1.read_volatile();
    let hcs2     = regs.capability.hcsparams2.read_volatile();
    let max_slots = hcs1.number_of_device_slots();
    let max_ports = hcs1.number_of_ports();

    // ── 2. Wait CNR clear ────────────────────────────────────────────────────
    if !wait_ms(|| !regs.operational.usbsts.read_volatile().controller_not_ready(), 100) {
        crate::bwarn!("usb", "xhci: CNR did not clear — aborting"); return None;
    }

    // ── 3. Halt (clear Run/Stop, wait HC Halted) ─────────────────────────────
    regs.operational.usbcmd.update_volatile(|c| { c.clear_run_stop(); });
    if !wait_ms(|| regs.operational.usbsts.read_volatile().hc_halted(), 100) {
        crate::bwarn!("usb", "xhci: HC did not halt — aborting"); return None;
    }

    // ── 4. Reset (HCRST, wait bit clears + CNR clears) ───────────────────────
    regs.operational.usbcmd.update_volatile(|c| { c.set_host_controller_reset(); });
    if !wait_ms(|| {
        !regs.operational.usbcmd.read_volatile().host_controller_reset()
        && !regs.operational.usbsts.read_volatile().controller_not_ready()
    }, 100) {
        crate::bwarn!("usb", "xhci: reset did not complete — aborting"); return None;
    }

    // ── 5. MaxSlotsEn ────────────────────────────────────────────────────────
    regs.operational.config.update_volatile(|c| { c.set_max_device_slots_enabled(max_slots); });

    // ── 6. DCBAA ─────────────────────────────────────────────────────────────
    // One page (4 KiB) = 512 u64 entries; slot 0 reserved for scratchpad.
    let dcbaa = match dma::alloc(1) {
        Some(r) => r,
        None => { crate::bwarn!("usb", "xhci: dcbaa alloc failed"); return None; }
    };
    regs.operational.dcbaap.update_volatile(|r| { r.set(dcbaa.phys.as_u64()); });

    // ── 7. Scratchpad ────────────────────────────────────────────────────────
    let n_scratch = hcs2.max_scratchpad_buffers();   // u32, combines hi+lo
    let (scratchpad, scratch_bufs) = if n_scratch > 0 {
        let array = match dma::alloc(1) {
            Some(r) => r,
            None => { crate::bwarn!("usb", "xhci: scratchpad array alloc failed"); return None; }
        };
        let mut bufs: Vec<DmaRegion> = Vec::with_capacity(n_scratch as usize);
        for i in 0..(n_scratch as usize) {
            let b = match dma::alloc(1) {
                Some(r) => r,
                None => { crate::bwarn!("usb", "xhci: scratchpad buf alloc failed"); return None; }
            };
            // SAFETY: `array` is DMA-zeroed, properly aligned, we stay in bounds.
            unsafe {
                array.virt.as_mut_ptr::<u64>().add(i).write_volatile(b.phys.as_u64());
            }
            bufs.push(b);
        }
        // DCBAA[0] = scratchpad array physical address.
        unsafe {
            dcbaa.virt.as_mut_ptr::<u64>().write_volatile(array.phys.as_u64());
        }
        (Some(array), bufs)
    } else {
        (None, Vec::new())
    };

    // ── 8. Command ring ───────────────────────────────────────────────────────
    // One page = 4096 / 16 = 256 TRB slots; all zeroed (cycle 0 = HC-owned).
    let cmd_ring = match dma::alloc(1) {
        Some(r) => r,
        None => { crate::bwarn!("usb", "xhci: cmd ring alloc failed"); return None; }
    };
    // CRCR: set pointer + initial producer cycle state (1).
    // set_command_ring_pointer requires 64-byte alignment; our DMA frame is
    // page-aligned (4 KiB), so the assertion always passes.
    regs.operational.crcr.update_volatile(|r| {
        r.set_command_ring_pointer(cmd_ring.phys.as_u64());
        r.set_ring_cycle_state();   // wo_bit → set_ring_cycle_state() only (no getter)
    });

    // ── 9. Event ring + ERST ─────────────────────────────────────────────────
    let event_ring = match dma::alloc(1) {
        Some(r) => r,
        None => { crate::bwarn!("usb", "xhci: event ring alloc failed"); return None; }
    };
    let erst = match dma::alloc(1) {
        Some(r) => r,
        None => { crate::bwarn!("usb", "xhci: erst alloc failed"); return None; }
    };

    // ERST[0]: u64 base address + u32 size (low 16 bits) + u32 reserved.
    // The segment has 256 TRBs (page / 16 bytes).
    unsafe {
        erst.virt.as_mut_ptr::<u64>().write_volatile(event_ring.phys.as_u64());
        erst.virt.as_mut_ptr::<u32>().add(2).write_volatile(256u32);
        erst.virt.as_mut_ptr::<u32>().add(3).write_volatile(0u32);
    }

    // Program interrupter 0: ERSTSZ → ERDP → ERSTBA (order matters; ERSTBA arms).
    {
        let mut int0 = regs.interrupter_register_set.interrupter_mut(0);
        int0.erstsz.update_volatile(|r| { r.set(1); });
        int0.erdp.update_volatile(|r| {
            r.set_event_ring_dequeue_pointer(event_ring.phys.as_u64());
        });
        int0.erstba.update_volatile(|r| { r.set(erst.phys.as_u64()); });
    }
    // Interrupt enable stays OFF — we poll.

    // ── 10. Run ───────────────────────────────────────────────────────────────
    regs.operational.usbcmd.update_volatile(|c| { c.set_run_stop(); });
    if !wait_ms(|| !regs.operational.usbsts.read_volatile().hc_halted(), 100) {
        crate::bwarn!("usb", "xhci: HC did not start running — aborting"); return None;
    }

    // ── 10b. Port Power (real hardware) ──────────────────────────────────────
    // After HCRST a controller that implements Port Power Control leaves its
    // root ports UNPOWERED (PORTSC.PP=0): no device ever asserts connect, so
    // enumeration sees nothing (`slots=0`) and the live-CD USB stick is never
    // found. QEMU auto-powers its ports (PP reads 1), so this is a no-op there
    // but essential on real machines. Set PP on every root port — preserving
    // every RW1C change bit (write 0 so the read-modify-write does not clear
    // them) — then wait the power-on-to-power-good settle before devices are
    // polled for connect. Real ports debounce after this; `media_bin`'s pump
    // (and the executor) re-scan for the late connect.
    for port in 1..=max_ports {
        regs.port_register_set.update_volatile_at((port - 1) as usize, |p| {
            p.portsc.set_port_power();
            p.portsc.set_0_connect_status_change();
            p.portsc.set_0_port_enabled_disabled();
            p.portsc.set_0_port_enabled_disabled_change();
            p.portsc.set_0_warm_port_reset_change();
            p.portsc.set_0_over_current_change();
            p.portsc.set_0_port_reset_change();
            p.portsc.set_0_port_link_state_change();
            p.portsc.set_0_port_config_error_change();
        });
    }
    // Power-good settle (xHCI PP→PWRGOOD is controller-specific; 20 ms covers
    // the common case — `wait_ms` with a never-true predicate just delays).
    wait_ms(|| false, 20);

    // ── 11. Log ───────────────────────────────────────────────────────────────
    crate::binfo!("usb", "xhci up slots={} ports={}", max_slots, max_ports);

    // ── 12. Build the controller handle ───────────────────────────────────────
    let mut x = Xhci {
        idx,
        regs,
        max_slots,
        max_ports,
        dcbaa,
        cmd_ring,
        event_ring,
        erst,
        scratchpad,
        scratch_bufs,
        cmd_cycle:     true,
        cmd_enqueue:   0,
        event_cycle:   true,
        event_dequeue: 0,
    };

    // ── 13. Install Link TRB + No-Op round-trip (Task 3) ─────────────────────
    ring::init_cmd_link(&x);
    ring::enqueue_cmd(&mut x, [0, 0, 0, 0], ring::TRB_NOOP_CMD);
    // Wait up to 50 ms for a Command Completion event with Success (code 1).
    let start = crate::boot::clock::elapsed_ms();
    let mut ok = false;
    while crate::boot::clock::elapsed_ms() - start < 50 {
        if let Some(ev) = ring::poll_event(&mut x) {
            if ring::trb_type(&ev) == ring::TRB_CMD_COMPLETION {
                ok = ring::completion_code(&ev) == 1;
                break;
            }
        }
    }
    if ok {
        crate::binfo!("usb", "noop ok");
    } else {
        crate::bwarn!("usb", "noop FAIL");
    }

    // ── Seed the worklist: one RootPortChanged per connected root port ───────
    // Enumeration itself runs on the first `poll()` after the executor starts —
    // init must not block on per-device control transfers.
    for port in 1..=x.max_ports {
        let connected = x.regs.port_register_set
            .read_volatile_at((port - 1) as usize)
            .portsc.current_connect_status();
        if connected {
            crate::usb::registry::push_action(
                crate::usb::registry::UsbAction::RootPortChanged { ctrl: idx, port },
            );
        }
    }

    Some(x)
}

/// Bring up EVERY xHCI controller in PCI (not just the first). A machine can
/// expose several — e.g. a Tiger Lake laptop has a Thunderbolt/USB4 xHCI plus a
/// PCH xHCI, and the USB-A ports (where a boot stick or keyboard sits) belong to
/// the PCH one while the first-by-bus is the empty Thunderbolt controller. Each
/// controller is tagged with its index and tracked in `usb::CTRLS`.
pub fn init() {
    let mut ctrls: Vec<Xhci> = Vec::new();
    for dev in crate::pci::devices() {
        if dev.class == 0x0C && dev.subclass == 0x03 && dev.prog_if == 0x30 {
            let idx = ctrls.len() as u8;
            if idx as usize >= crate::usb::registry::MAX_XHCI {
                crate::bwarn!("usb", "more than {} xHCI controllers — ignoring the rest",
                    crate::usb::registry::MAX_XHCI);
                break;
            }
            match bringup(dev, idx) {
                Some(x) => ctrls.push(x),
                None => crate::bwarn!("usb", "xhci controller {} bring-up failed", idx),
            }
        }
    }
    if ctrls.is_empty() {
        crate::bwarn!("usb", "no xhci controller — skipping");
        return;
    }
    crate::binfo!("usb", "{} xhci controller(s) up", ctrls.len());
    crate::usb::CTRLS.call_once(|| crate::sync::IrqMutex::new(ctrls));
}

pub fn poll() {
    let cell = match crate::usb::CTRLS.get() { Some(c) => c, None => return };
    let mut g = cell.lock();
    // Drain every controller's event ring (Transfer Events → slot handlers,
    // Port Status Change → worklist tagged with that controller's index).
    for x in g.iter_mut() {
        while let Some(ev) = ring::poll_event(x) {
            event::dispatch(x, ev);
        }
    }
    // Then drain the connect/disconnect worklist, routing each action to the
    // controller that raised it (actions carry their `ctrl` index).
    while let Some(a) = crate::usb::registry::pop_action() {
        let ci = a.ctrl() as usize;
        if let Some(x) = g.get_mut(ci) { handle_action(x, a); }
    }
}

/// Act on one worklist item: reset + enumerate newly-connected devices. Runs with
/// the controller (`x`) locked but the SLOTS lock free — `enumerate` only locks
/// SLOTS for its final `insert`, never across a command/event drain.
fn handle_action(x: &mut Xhci, a: crate::usb::registry::UsbAction) {
    use crate::usb::registry::UsbAction;
    match a {
        UsbAction::RootPortChanged { port: p, .. } => {
            let portsc = x.regs.port_register_set
                .read_volatile_at((p - 1) as usize).portsc;
            let connected = portsc.current_connect_status();
            // Clear the connect-status-change (CSC) RW1C bit, preserving the other
            // change bits (set_0_* keeps them 0 in the written value so a plain
            // read-modify-write does not accidentally clear them).
            x.regs.port_register_set.update_volatile_at((p - 1) as usize, |r| {
                r.portsc.clear_connect_status_change();
                r.portsc.set_0_port_enabled_disabled();
                r.portsc.set_0_port_enabled_disabled_change();
                r.portsc.set_0_warm_port_reset_change();
                r.portsc.set_0_over_current_change();
                r.portsc.set_0_port_reset_change();
                r.portsc.set_0_port_link_state_change();
                r.portsc.set_0_port_config_error_change();
            });
            let existing = crate::usb::registry::find_root(x.idx, p);
            match (connected, existing) {
                // Newly connected, not yet enumerated → reset + enumerate.
                (true, None) => {
                    if let Some(speed) = crate::usb::device::reset_root_port(x, p) {
                        let loc = crate::usb::device::Location {
                            root_port: p, route: 0, tier: 0, speed,
                            parent_slot: 0, parent_port: 0, tt: false,
                        };
                        let _ = crate::usb::device::enumerate(x, loc);
                    }
                }
                // Disconnected with a live root slot → tear it down (+ children).
                (false, Some(slot)) => { crate::usb::registry::teardown(x, slot); }
                // Already-present connect, or empty disconnect: nothing to do.
                _ => {}
            }
        }
        UsbAction::HubPortChanged { hub_slot, port, .. } => {
            crate::usb::hub::handle_port(x, hub_slot, port);
        }
    }
}
