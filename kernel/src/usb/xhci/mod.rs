//! xHCI host controller driver — Task 3: TRB ring abstraction + No-Op round-trip.
//!
//! Sequence (xHCI 1.2 §4.2): find/enable PCI device, wait CNR, halt, reset,
//! set MaxSlotsEn, allocate DCBAA + scratchpad + command ring + event ring,
//! program registers, run, issue No-Op command, verify Command Completion event.
pub mod regs;
pub mod ring;
pub mod event;

use crate::memory::dma::{self, DmaRegion};
use crate::pci;
use regs::HhdmMapper;
use alloc::vec::Vec;

/// xHCI controller state — holds DMA regions and ring bookkeeping.
pub struct Xhci {
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

/// Bring up the xHCI controller. Non-fatal: logs a warning and returns on any
/// error so that a missing/broken controller does not hang the system.
pub fn init() {
    // ── 1. PCI: find, enable, map BAR0 ──────────────────────────────────────
    let dev = match pci::find_class(0x0C, 0x03, 0x30) {
        Some(d) => d,
        None => { crate::bwarn!("usb", "no xhci controller — skipping"); return; }
    };
    dev.enable_mmio();
    dev.enable_bus_master();
    let (base, _size) = match dev.bar(0) {
        Some(pci::Bar::Memory64 { address, size, .. }) => (address, size as usize),
        Some(pci::Bar::Memory32 { address, size, .. }) => (address as u64, size as usize),
        other => { crate::bwarn!("usb", "xhci bar0 unexpected: {:?}", other); return; }
    };

    // SAFETY: `base` is the xHCI BAR0 physical address; HhdmMapper maps each
    // register block into the HHDM virtual window.
    let mut regs = unsafe {
        ::xhci::Registers::new(base as usize, HhdmMapper)
    };

    let hcs1     = regs.capability.hcsparams1.read_volatile();
    let hcs2     = regs.capability.hcsparams2.read_volatile();
    let max_slots = hcs1.number_of_device_slots();
    let max_ports = hcs1.number_of_ports();

    // ── 2. Wait CNR clear ────────────────────────────────────────────────────
    if !wait_ms(|| !regs.operational.usbsts.read_volatile().controller_not_ready(), 100) {
        crate::bwarn!("usb", "xhci: CNR did not clear — aborting"); return;
    }

    // ── 3. Halt (clear Run/Stop, wait HC Halted) ─────────────────────────────
    regs.operational.usbcmd.update_volatile(|c| { c.clear_run_stop(); });
    if !wait_ms(|| regs.operational.usbsts.read_volatile().hc_halted(), 100) {
        crate::bwarn!("usb", "xhci: HC did not halt — aborting"); return;
    }

    // ── 4. Reset (HCRST, wait bit clears + CNR clears) ───────────────────────
    regs.operational.usbcmd.update_volatile(|c| { c.set_host_controller_reset(); });
    if !wait_ms(|| {
        !regs.operational.usbcmd.read_volatile().host_controller_reset()
        && !regs.operational.usbsts.read_volatile().controller_not_ready()
    }, 100) {
        crate::bwarn!("usb", "xhci: reset did not complete — aborting"); return;
    }

    // ── 5. MaxSlotsEn ────────────────────────────────────────────────────────
    regs.operational.config.update_volatile(|c| { c.set_max_device_slots_enabled(max_slots); });

    // ── 6. DCBAA ─────────────────────────────────────────────────────────────
    // One page (4 KiB) = 512 u64 entries; slot 0 reserved for scratchpad.
    let dcbaa = match dma::alloc(1) {
        Some(r) => r,
        None => { crate::bwarn!("usb", "xhci: dcbaa alloc failed"); return; }
    };
    regs.operational.dcbaap.update_volatile(|r| { r.set(dcbaa.phys.as_u64()); });

    // ── 7. Scratchpad ────────────────────────────────────────────────────────
    let n_scratch = hcs2.max_scratchpad_buffers();   // u32, combines hi+lo
    let (scratchpad, scratch_bufs) = if n_scratch > 0 {
        let array = match dma::alloc(1) {
            Some(r) => r,
            None => { crate::bwarn!("usb", "xhci: scratchpad array alloc failed"); return; }
        };
        let mut bufs: Vec<DmaRegion> = Vec::with_capacity(n_scratch as usize);
        for i in 0..(n_scratch as usize) {
            let b = match dma::alloc(1) {
                Some(r) => r,
                None => { crate::bwarn!("usb", "xhci: scratchpad buf alloc failed"); return; }
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
        None => { crate::bwarn!("usb", "xhci: cmd ring alloc failed"); return; }
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
        None => { crate::bwarn!("usb", "xhci: event ring alloc failed"); return; }
    };
    let erst = match dma::alloc(1) {
        Some(r) => r,
        None => { crate::bwarn!("usb", "xhci: erst alloc failed"); return; }
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
        crate::bwarn!("usb", "xhci: HC did not start running — aborting"); return;
    }

    // ── 11. Log ───────────────────────────────────────────────────────────────
    crate::binfo!("usb", "xhci up slots={} ports={}", max_slots, max_ports);

    // ── 12. Store global handle ───────────────────────────────────────────────
    let mut x = Xhci {
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

    // ── Root port scan + reset (Task 4) ──────────────────────────────────────
    if let Some(port) = crate::usb::device::scan_ports(&mut x) {
        // ── Enable Slot + Address Device (Task 5) ─────────────────────────
        if let Some(mut dev) = crate::usb::device::address_device(&mut x, &port) {
            // ── Task 6: Read Device Descriptor (EP0 control-IN) ──────────
            crate::usb::device::read_device_descriptor(&mut x, &mut dev);
            // ── Task 7: Config descriptor + HID detect + SET_CONFIGURATION ─
            let _kb = crate::usb::device::configure(&mut x, &mut dev);
            if let Some(kb) = _kb {
                crate::usb::KBD.call_once(|| crate::sync::IrqMutex::new(Some(kb)));
                // ── Task 8: Configure EP + boot protocol + queue first report ─
                if let Some(st) = crate::usb::hid::configure_endpoint(&mut x, &mut dev, &kb) {
                    crate::usb::HID.call_once(|| crate::sync::IrqMutex::new(Some(st)));
                }
            }
            crate::usb::DEVICE.call_once(|| crate::sync::IrqMutex::new(Some(dev)));
        }
    }

    crate::usb::CTRL.call_once(|| crate::sync::IrqMutex::new(Some(x)));
}

pub fn poll() {
    let ctrl_cell = match crate::usb::CTRL.get() { Some(c) => c, None => return };
    let mut g = ctrl_cell.lock();
    let x = match g.as_mut() { Some(x) => x, None => return };
    // Drain every pending event through the central dispatcher (routes Transfer
    // Events to slot handlers + Port Status Change to the worklist).
    while let Some(ev) = ring::poll_event(x) {
        event::dispatch(x, ev);
    }
}
