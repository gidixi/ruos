//! Loopback device: smoltcp's built-in zero-config medium-ip
//! device. Trivial wrapper for clarity.

pub use smoltcp::phy::Loopback;

pub fn new() -> Loopback {
    Loopback::new(smoltcp::phy::Medium::Ip)
}
