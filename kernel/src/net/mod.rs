//! Network stack — smoltcp on a Loopback device. NIC real driver
//! arrives at Step 14; today only 127.0.0.1/8 traffic.

pub mod loopback;
pub mod sockets;

use spin::Mutex;
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::time::Instant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr};
pub struct NetState {
    pub iface: Interface,
    pub device: loopback::Loopback,
    pub sockets: SocketSet<'static>,
}

pub static NET: Mutex<Option<NetState>> = Mutex::new(None);

pub fn init() {
    let mut device = loopback::new();
    let now = Instant::from_millis(crate::timer::ticks() as i64 * 10);

    let config = Config::new(HardwareAddress::Ip);
    let mut iface = Interface::new(config, &mut device, now);
    iface.update_ip_addrs(|addrs| {
        addrs.push(IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8)).unwrap();
    });

    let sockets = SocketSet::new(alloc::vec::Vec::new());
    *NET.lock() = Some(NetState { iface, device, sockets });
}

/// Called periodically by `net_poll_task` (every 10 ms).
pub fn poll() {
    use x86_64::instructions::interrupts::without_interrupts;
    without_interrupts(|| {
        let mut g = NET.lock();
        if let Some(net) = g.as_mut() {
            let now = Instant::from_millis(crate::timer::ticks() as i64 * 10);
            let _ = net.iface.poll(now, &mut net.device, &mut net.sockets);
        }
    });
}
