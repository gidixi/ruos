//! USB hub class driver. (Task 1: state struct only; behavior added in Task 4.)
use crate::memory::dma::DmaRegion;
use crate::usb::xhci::Xhci;

/// Running state for a configured hub.
pub struct HubState {
    pub dci: u8,
    pub nbr_ports: u8,
    pub int_ring: DmaRegion,
    pub int_enqueue: usize,
    pub int_cycle: bool,
    pub change_buf: DmaRegion,
}

/// Handle a hub status-change interrupt completion. (Task 4 implements; stub now.)
pub fn on_status(_x: &mut Xhci, _slot: u8, _st: &mut HubState) {}
