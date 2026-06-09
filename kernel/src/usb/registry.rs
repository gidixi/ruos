//! USB slot registry + connect/disconnect action worklist.
//!
//! Tracks every addressed slot across ALL xHCI controllers so hot-plug can
//! add/remove devices and route events by slot. Owns each slot's DMA so teardown
//! frees it.
//!
//! MULTI-CONTROLLER: slot ids are assigned per-controller by hardware (Enable
//! Slot), so controller 0 and controller 1 both use slot 1. The registry is
//! therefore keyed by `(ctrl, slot)` — `gidx(ctrl, slot)` flattens that into the
//! backing array. Every registry call takes the controller index (handlers
//! already hold the controller `x`, whose `idx` field is that index).
//!
//! LOCK DISCIPLINE: never hold the SLOTS lock across a command/`wait_cmd`/event
//! drain (those dispatch events that re-lock SLOTS → deadlock). Collect what you
//! need under the lock, release it, THEN issue controller commands.
use crate::memory::dma::{self, DmaRegion};
use crate::sync::IrqMutex;
use crate::usb::device::UsbDevice;
use alloc::collections::VecDeque;

pub const MAX_SLOTS: usize = 256;
/// Max xHCI host controllers tracked concurrently (e.g. a Tiger Lake laptop has
/// a Thunderbolt xHCI + a PCH xHCI). Slots are namespaced by controller.
pub const MAX_XHCI: usize = 4;
const TOTAL: usize = MAX_SLOTS * MAX_XHCI;

#[inline]
fn gidx(ctrl: u8, slot: u8) -> usize { (ctrl as usize) * MAX_SLOTS + slot as usize }

pub enum SlotKind {
    Hub(crate::usb::hub::HubState),
    Keyboard(crate::usb::hid::HidState),
    Mouse(crate::usb::hid::HidState),
    Msc(crate::usb::msc::MscState),
    Wifi(crate::usb::wifi::WifiState),
    Other,
}

pub struct SlotEntry {
    pub kind: SlotKind,
    pub dev: UsbDevice,
    pub ctrl: u8,        // owning xHCI controller index
    pub root_port: u8,
    pub parent_slot: u8, // 0 = root
    pub parent_port: u8, // hub port (0 = root)
    pub route: u32,
    pub tier: u8,
    pub speed: u8,
}

const NONE: Option<SlotEntry> = None;
static SLOTS: IrqMutex<[Option<SlotEntry>; TOTAL]> = IrqMutex::new([NONE; TOTAL]);

#[derive(Clone, Copy)]
pub enum UsbAction {
    RootPortChanged { ctrl: u8, port: u8 },
    HubPortChanged { ctrl: u8, hub_slot: u8, port: u8 },
}

impl UsbAction {
    pub fn ctrl(&self) -> u8 {
        match self { UsbAction::RootPortChanged { ctrl, .. } | UsbAction::HubPortChanged { ctrl, .. } => *ctrl }
    }
}

static WORK: IrqMutex<VecDeque<UsbAction>> = IrqMutex::new(VecDeque::new());

pub fn push_action(a: UsbAction) { WORK.lock().push_back(a); }
pub fn pop_action() -> Option<UsbAction> { WORK.lock().pop_front() }

pub fn insert(ctrl: u8, slot: u8, e: SlotEntry) { SLOTS.lock()[gidx(ctrl, slot)] = Some(e); }

/// First mass-storage device as `(ctrl, slot)`, if any. Used by `media_bin` to
/// mount `/bin` off-boot from a USB stick.
pub fn first_msc_slot() -> Option<(u8, u8)> {
    let g = SLOTS.lock();
    for c in 0..MAX_XHCI as u8 {
        for s in 1..MAX_SLOTS as u16 {
            if let Some(e) = g[gidx(c, s as u8)].as_ref() {
                if matches!(e.kind, SlotKind::Msc(_)) { return Some((c, s as u8)); }
            }
        }
    }
    None
}

/// Copy the MSC running state OUT of the registry (lock released on return) so
/// the caller can run bulk transfers without holding SLOTS (the
/// `wait_for`→dispatch→`with_slot` re-lock would deadlock). `MscState` is `Copy`.
pub fn msc_state(ctrl: u8, slot: u8) -> Option<crate::usb::msc::MscState> {
    let g = SLOTS.lock();
    match g[gidx(ctrl, slot)].as_ref().map(|e| &e.kind) {
        Some(SlotKind::Msc(st)) => Some(*st),
        _ => None,
    }
}

/// Write back an advanced `MscState` (ring cursors) after a transfer.
pub fn set_msc_state(ctrl: u8, slot: u8, st: crate::usb::msc::MscState) {
    let mut g = SLOTS.lock();
    if let Some(e) = g[gidx(ctrl, slot)].as_mut() {
        if let SlotKind::Msc(s) = &mut e.kind { *s = st; }
    }
}

/// Run `f` against the slot entry (if present) while holding the lock. Do NOT
/// issue controller commands or drain events from inside `f` (lock held).
pub fn with_slot<R>(ctrl: u8, slot: u8, f: impl FnOnce(&mut SlotEntry) -> R) -> Option<R> {
    let mut g = SLOTS.lock();
    g[gidx(ctrl, slot)].as_mut().map(f)
}

/// Route a Transfer Event to the slot's class handler. Holds SLOTS only while
/// the handler runs; handlers must not lock SLOTS or drain events (invariant).
/// The controller is `x.idx`.
pub fn dispatch_transfer(x: &mut crate::usb::xhci::Xhci, slot: u8, dci: u8) {
    let ctrl = x.idx;
    with_slot(ctrl, slot, |e| match &mut e.kind {
        SlotKind::Keyboard(st) if st.dci == dci => crate::usb::hid::on_report(x, st),
        SlotKind::Mouse(st) if st.dci == dci => crate::usb::hid::on_report_mouse(x, st),
        SlotKind::Hub(hs) if hs.dci == dci => crate::usb::hub::on_status(x, slot, hs),
        _ => {}
    });
}

/// Diagnostic snapshot of every enumerated slot: `(slot, root_port, speed,
/// kind)`. Used by the `usb-probe` boot diagnostic.
#[cfg(feature = "usb-probe")]
pub fn probe_dump() -> alloc::vec::Vec<(u8, u8, u8, &'static str)> {
    let g = SLOTS.lock();
    let mut v = alloc::vec::Vec::new();
    for c in 0..MAX_XHCI as u8 {
        for s in 1..MAX_SLOTS as u16 {
            if let Some(e) = g[gidx(c, s as u8)].as_ref() {
                v.push((s as u8, e.root_port, e.speed, kind_name(&e.kind)));
            }
        }
    }
    v
}

fn kind_name(k: &SlotKind) -> &'static str {
    match k {
        SlotKind::Hub(_) => "Hub",
        SlotKind::Keyboard(_) => "Keyboard",
        SlotKind::Mouse(_) => "Mouse",
        SlotKind::Msc(_) => "Msc",
        SlotKind::Wifi(_) => "Wifi",
        SlotKind::Other => "Other",
    }
}

/// Boot-time device summary: `(total enumerated slots, mass-storage slots)`.
pub fn slot_summary() -> (usize, usize) {
    let g = SLOTS.lock();
    let mut total = 0; let mut msc = 0;
    for c in 0..MAX_XHCI as u8 {
        for s in 1..MAX_SLOTS as u16 {
            if let Some(e) = g[gidx(c, s as u8)].as_ref() {
                total += 1;
                if matches!(e.kind, SlotKind::Msc(_)) { msc += 1; }
            }
        }
    }
    (total, msc)
}

/// Per-slot kind list `(ctrl, slot, root_port, kind)` for boot diagnostics.
pub fn dump_slots() -> alloc::vec::Vec<(u8, u8, u8, &'static str)> {
    let g = SLOTS.lock();
    let mut v = alloc::vec::Vec::new();
    for c in 0..MAX_XHCI as u8 {
        for s in 1..MAX_SLOTS as u16 {
            if let Some(e) = g[gidx(c, s as u8)].as_ref() {
                v.push((c, s as u8, e.root_port, kind_name(&e.kind)));
            }
        }
    }
    v
}

/// Root device on `ctrl`'s root-hub port `port`, if already enumerated.
pub fn find_root(ctrl: u8, port: u8) -> Option<u8> {
    let g = SLOTS.lock();
    for s in 1..MAX_SLOTS as u16 {
        if let Some(e) = g[gidx(ctrl, s as u8)].as_ref() {
            if e.parent_slot == 0 && e.root_port == port { return Some(s as u8); }
        }
    }
    None
}

/// First child slot hanging off (`ctrl`, parent_slot, port), if any.
pub fn find_child(ctrl: u8, parent_slot: u8, port: u8) -> Option<u8> {
    let g = SLOTS.lock();
    for s in 1..MAX_SLOTS as u16 {
        if let Some(e) = g[gidx(ctrl, s as u8)].as_ref() {
            if e.parent_slot == parent_slot && e.parent_port == port { return Some(s as u8); }
        }
    }
    None
}

/// Direct children of (`ctrl`, slot) (for recursive teardown).
fn children_of(ctrl: u8, slot: u8) -> alloc::vec::Vec<u8> {
    let g = SLOTS.lock();
    let mut v = alloc::vec::Vec::new();
    for s in 1..MAX_SLOTS as u16 {
        if let Some(e) = g[gidx(ctrl, s as u8)].as_ref() {
            if e.parent_slot == slot { v.push(s as u8); }
        }
    }
    v
}

/// Tear a slot down: recursively remove children first, then Disable Slot,
/// clear DCBAA, free DMA, drop the entry. `x` = controller (`x.idx` = ctrl).
pub fn teardown(x: &mut crate::usb::xhci::Xhci, slot: u8) {
    let ctrl = x.idx;
    // Children first (lock released before recursing/commanding).
    for c in children_of(ctrl, slot) { teardown(x, c); }
    // Remove the entry under the lock, take ownership of its DMA to free after.
    let entry = SLOTS.lock()[gidx(ctrl, slot)].take();
    let entry = match entry { Some(e) => e, None => return };
    // Disable Slot (cmd type 10): slot id in word3 bits 24..31. Only free DMA on
    // success (code 1); on failure leak it to avoid a use-after-free.
    crate::usb::xhci::ring::enqueue_cmd(x, [0, 0, 0, (slot as u32) << 24], 10);
    let ok = matches!(
        crate::usb::xhci::ring::wait_cmd(x),
        Some(ev) if crate::usb::xhci::ring::completion_code(&ev) == 1
    );
    // Clear DCBAA[slot] regardless (the slot is gone from our tracking).
    unsafe { x.dcbaa.virt.as_mut_ptr::<u64>().add(slot as usize).write_volatile(0); }
    if ok {
        dma::dealloc(entry.dev.ep0_ring);
        dma::dealloc(entry.dev.input_ctx);
        dma::dealloc(entry.dev.dev_ctx);
        match entry.kind {
            SlotKind::Hub(h) => { dma::dealloc(h.int_ring); dma::dealloc(h.change_buf); }
            SlotKind::Keyboard(k) | SlotKind::Mouse(k) => {
                dma::dealloc(k.int_ring); dma::dealloc(k.report);
            }
            SlotKind::Msc(m) => {
                dma::dealloc(m.ring_in); dma::dealloc(m.ring_out); dma::dealloc(m.data);
            }
            SlotKind::Wifi(w) => { dma::dealloc(w.ring_in); dma::dealloc(w.ring_out); dma::dealloc(w.data); }
            SlotKind::Other => {}
        }
        crate::binfo!("usb", "teardown ctrl={} slot={}", ctrl, slot);
    } else {
        crate::bwarn!("usb", "teardown ctrl={} slot={} disable failed — leaking DMA to avoid UAF", ctrl, slot);
    }
}
