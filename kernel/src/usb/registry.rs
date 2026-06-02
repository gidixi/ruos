//! USB slot registry + connect/disconnect action worklist.
//!
//! Replaces the MVP's DEVICE/KBD/HID singletons: tracks every addressed slot so
//! hot-plug can add/remove devices and route events by slot. Owns each slot's
//! DMA so teardown frees it.
//!
//! LOCK DISCIPLINE: never hold the SLOTS lock across a command/`wait_cmd`/event
//! drain (those dispatch events that re-lock SLOTS → deadlock). Collect what you
//! need under the lock, release it, THEN issue controller commands.
use crate::memory::dma::{self, DmaRegion};
use crate::sync::IrqMutex;
use crate::usb::device::UsbDevice;
use alloc::collections::VecDeque;

pub const MAX_SLOTS: usize = 256;

pub enum SlotKind {
    Hub(crate::usb::hub::HubState),
    Keyboard(crate::usb::hid::HidState),
    Other,
}

pub struct SlotEntry {
    pub kind: SlotKind,
    pub dev: UsbDevice,
    pub root_port: u8,
    pub parent_slot: u8, // 0 = root
    pub parent_port: u8, // hub port (0 = root)
    pub route: u32,
    pub tier: u8,
    pub speed: u8,
}

const NONE: Option<SlotEntry> = None;
static SLOTS: IrqMutex<[Option<SlotEntry>; MAX_SLOTS]> = IrqMutex::new([NONE; MAX_SLOTS]);

#[derive(Clone, Copy)]
pub enum UsbAction {
    RootPortChanged(u8),
    HubPortChanged { hub_slot: u8, port: u8 },
}
static WORK: IrqMutex<VecDeque<UsbAction>> = IrqMutex::new(VecDeque::new());

pub fn push_action(a: UsbAction) { WORK.lock().push_back(a); }
pub fn pop_action() -> Option<UsbAction> { WORK.lock().pop_front() }

pub fn insert(slot: u8, e: SlotEntry) { SLOTS.lock()[slot as usize] = Some(e); }

/// Run `f` against the slot entry (if present) while holding the lock. Do NOT
/// issue controller commands or drain events from inside `f` (lock held).
pub fn with_slot<R>(slot: u8, f: impl FnOnce(&mut SlotEntry) -> R) -> Option<R> {
    let mut g = SLOTS.lock();
    g[slot as usize].as_mut().map(f)
}

/// Route a Transfer Event to the slot's class handler. Holds SLOTS only while
/// the handler runs; handlers must not lock SLOTS or drain events (invariant).
pub fn dispatch_transfer(x: &mut crate::usb::xhci::Xhci, slot: u8, dci: u8) {
    with_slot(slot, |e| match &mut e.kind {
        SlotKind::Keyboard(st) if st.dci == dci => crate::usb::hid::on_report(x, st),
        SlotKind::Hub(hs) if hs.dci == dci => crate::usb::hub::on_status(x, slot, hs),
        _ => {}
    });
}

/// Root device on root-hub port `port`, if one is already enumerated. A root
/// device registers with `parent_slot == 0` (root-attached) and `root_port`
/// set — distinct from `find_child(0, port)`, which keys on `parent_port`.
pub fn find_root(port: u8) -> Option<u8> {
    let g = SLOTS.lock();
    for i in 1..MAX_SLOTS {
        if let Some(e) = g[i].as_ref() {
            if e.parent_slot == 0 && e.root_port == port {
                return Some(i as u8);
            }
        }
    }
    None
}

/// First child slot hanging off (parent_slot, port), if any.
pub fn find_child(parent_slot: u8, port: u8) -> Option<u8> {
    let g = SLOTS.lock();
    for i in 1..MAX_SLOTS {
        if let Some(e) = g[i].as_ref() {
            if e.parent_slot == parent_slot && e.parent_port == port {
                return Some(i as u8);
            }
        }
    }
    None
}

/// Direct children of `slot` (for recursive teardown). Returns a small list.
fn children_of(slot: u8) -> alloc::vec::Vec<u8> {
    let g = SLOTS.lock();
    let mut v = alloc::vec::Vec::new();
    for i in 1..MAX_SLOTS {
        if let Some(e) = g[i].as_ref() { if e.parent_slot == slot { v.push(i as u8); } }
    }
    v
}

/// Tear a slot down: recursively remove children first, then Disable Slot,
/// clear DCBAA, free DMA, drop the entry. `x` = controller.
pub fn teardown(x: &mut crate::usb::xhci::Xhci, slot: u8) {
    // Children first (lock released before recursing/commanding).
    for c in children_of(slot) { teardown(x, c); }
    // Remove the entry under the lock, take ownership of its DMA to free after.
    let entry = SLOTS.lock()[slot as usize].take();
    let entry = match entry { Some(e) => e, None => return };
    // Disable Slot (cmd type 10): slot id in word3 bits 24..31.
    crate::usb::xhci::ring::enqueue_cmd(x, [0, 0, 0, (slot as u32) << 24], 10);
    let _ = crate::usb::xhci::ring::wait_cmd(x); // best-effort; ignore code
    // Clear DCBAA[slot].
    unsafe { x.dcbaa.virt.as_mut_ptr::<u64>().add(slot as usize).write_volatile(0); }
    // Free DMA: dev contexts/ring + kind-specific.
    dma::dealloc(entry.dev.ep0_ring);
    dma::dealloc(entry.dev.input_ctx);
    dma::dealloc(entry.dev.dev_ctx);
    match entry.kind {
        SlotKind::Hub(h) => { dma::dealloc(h.int_ring); dma::dealloc(h.change_buf); }
        SlotKind::Keyboard(k) => { dma::dealloc(k.int_ring); dma::dealloc(k.report); }
        SlotKind::Other => {}
    }
    crate::binfo!("usb", "teardown slot={}", slot);
}
