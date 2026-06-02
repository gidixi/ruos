//! Central event-ring dispatch. Every event TRB flows through `dispatch`;
//! `wait_for` routes non-matching events here instead of dropping them, which
//! fixes the MVP's event-loss and makes nested enumeration + hot-plug safe.
use super::Xhci;
use super::ring;
use crate::usb::registry::{self, UsbAction};

/// Route one event TRB to its handler / the worklist.
pub fn dispatch(x: &mut Xhci, ev: [u32; 4]) {
    match ring::trb_type(&ev) {
        32 => { // Transfer Event
            let slot = ((ev[3] >> 24) & 0xFF) as u8;
            let dci  = ((ev[3] >> 16) & 0x1F) as u8;
            registry::dispatch_transfer(x, slot, dci);
        }
        34 => { // Port Status Change Event: root port = word0 bits 24..31
            let port = ((ev[0] >> 24) & 0xFF) as u8;
            registry::push_action(UsbAction::RootPortChanged(port));
        }
        _ => {} // Command Completion: handled by wait_for's predicate; else ignore
    }
}

/// Poll the event ring with a bounded deadline (ms); return the first TRB
/// matching `pred`, dispatching every other TRB so nothing is lost.
pub fn wait_for(x: &mut Xhci, ms: u64, pred: impl Fn(&[u32; 4]) -> bool) -> Option<[u32; 4]> {
    let start = crate::boot::clock::elapsed_ms();
    while crate::boot::clock::elapsed_ms() - start < ms {
        if let Some(ev) = ring::poll_event(x) {
            if pred(&ev) { return Some(ev); }
            dispatch(x, ev);
        }
        core::hint::spin_loop();
    }
    None
}
