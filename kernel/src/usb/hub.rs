//! USB hub class driver. (Task 1: state struct only; behavior added in Task 4.)
use crate::memory::dma::DmaRegion;

/// Running state for a configured hub.
pub struct HubState {
    pub dci: u8,
    pub nbr_ports: u8,
    pub int_ring: DmaRegion,
    pub int_enqueue: usize,
    pub int_cycle: bool,
    pub change_buf: DmaRegion,
}
