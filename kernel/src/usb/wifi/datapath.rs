//! WiFi datapath decoupling queues (SP-WIFI-5).
//!
//! smoltcp's `iface.poll()` runs under the `NET` lock; USB bulk transfers run
//! under the `CTRLS` lock (interrupts masked, busy-poll). Those two locks must
//! NEVER nest. These two bounded queues are the only thing the USB side
//! (`wifi_poll_task`/`poll_io`, holds CTRLS) and the smoltcp side (`WifiPhy` via
//! `net::poll`, holds NET) share — and they are **leaf locks**: push/pop the
//! `Vec` and drop the guard immediately, never call USB/smoltcp/NET/CTRLS while
//! holding one. No cycle can form → no deadlock.
//!
//! Frames here are plain **Ethernet-II** (802.3); all 802.11/CCMP framing lives
//! on the USB side. `ATTACHED` gates the datapath live (set after the WPA2 4-way
//! succeeds and the smoltcp iface is built).

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use crate::sync::IrqMutex;

const RX_DEPTH: usize = 64;
const TX_DEPTH: usize = 64;

/// Ethernet frames decapsulated from RX 802.11 data frames, awaiting smoltcp.
static WIFI_RX: IrqMutex<VecDeque<Vec<u8>>> = IrqMutex::new(VecDeque::new());
/// Ethernet frames smoltcp produced, awaiting encap + TX on the radio.
static WIFI_TX: IrqMutex<VecDeque<Vec<u8>>> = IrqMutex::new(VecDeque::new());
/// Datapath live (smoltcp iface up + WPA2 keys installed).
static ATTACHED: AtomicBool = AtomicBool::new(false);

#[inline] pub fn is_attached() -> bool { ATTACHED.load(Ordering::Acquire) }
#[inline] pub fn set_attached(v: bool) { ATTACHED.store(v, Ordering::Release); }

/// Queue a decapsulated RX Ethernet frame for smoltcp. Drop-oldest on overflow
/// (keep liveness; a stale ingress frame matters less than blocking).
pub fn rx_push(frame: Vec<u8>) {
    let mut q = WIFI_RX.lock();
    if q.len() >= RX_DEPTH { q.pop_front(); }
    q.push_back(frame);
}

/// Pop the next RX Ethernet frame for smoltcp (None = empty, NOT a HW poll).
pub fn rx_pop() -> Option<Vec<u8>> {
    WIFI_RX.lock().pop_front()
}

/// Queue an egress Ethernet frame from smoltcp. Returns false if the queue is
/// full (the frame is dropped; smoltcp/TCP will retransmit).
pub fn tx_push(frame: Vec<u8>) -> bool {
    let mut q = WIFI_TX.lock();
    if q.len() >= TX_DEPTH { return false; }
    q.push_back(frame);
    true
}

/// Pop the next egress Ethernet frame for the radio (None = empty).
pub fn tx_pop() -> Option<Vec<u8>> {
    WIFI_TX.lock().pop_front()
}

/// Drop all queued frames + mark the datapath down (e.g. on disconnect).
#[allow(dead_code)]
pub fn reset() {
    set_attached(false);
    WIFI_RX.lock().clear();
    WIFI_TX.lock().clear();
}
