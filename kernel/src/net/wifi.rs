//! smoltcp `phy::Device` for the RTL8188EU WiFi link (SP-WIFI-5).
//!
//! This Device does ZERO USB I/O. It moves Ethernet frames to/from the two
//! global datapath queues (`usb::wifi::datapath`); the actual radio I/O +
//! 802.11/CCMP framing happen on the USB side (`wifi::poll_io`, under CTRLS).
//! Keeping this Device queue-only is what lets `net::poll` drive it under NET
//! without ever reaching the CTRLS lock — the two locks never nest.

use alloc::vec::Vec;
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

use crate::usb::wifi::datapath;

/// Ethernet MTU advertised to smoltcp (the 802.11/CCMP overhead is added on the
/// USB side and stays under the 4096-byte scratch page).
const WIFI_MTU: usize = 1500;

/// Stateless smoltcp Device backed by the WIFI_RX/WIFI_TX queues.
pub struct WifiPhy;

pub struct WifiRxToken(Vec<u8>);
pub struct WifiTxToken;

impl RxToken for WifiRxToken {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, f: F) -> R {
        let mut data = self.0;
        f(&mut data)
    }
}

impl TxToken for WifiTxToken {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        // smoltcp fills `buf` with a complete Ethernet-II frame; we enqueue it
        // for the radio. The closure is always invoked (smoltcp needs its
        // result) even when the queue is full and the frame is dropped.
        let mut buf = alloc::vec![0u8; len];
        let r = f(&mut buf);
        if !datapath::tx_push(buf) {
            crate::bwarn!("net", "wifi: tx queue full, dropping");
        }
        r
    }
}

impl Device for WifiPhy {
    type RxToken<'a> = WifiRxToken where Self: 'a;
    type TxToken<'a> = WifiTxToken where Self: 'a;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = WIFI_MTU;
        caps
    }

    fn receive(&mut self, _ts: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // None = the ingress queue is empty, NOT a hardware poll (the explicit
        // USB decoupling: wifi_poll_task fills WIFI_RX off the CTRLS path).
        datapath::rx_pop().map(|frame| (WifiRxToken(frame), WifiTxToken))
    }

    fn transmit(&mut self, _ts: Instant) -> Option<Self::TxToken<'_>> {
        Some(WifiTxToken) // always-Some; queue-full drop is in TxToken::consume
    }
}
