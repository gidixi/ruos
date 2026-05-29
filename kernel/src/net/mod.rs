//! Network stack — smoltcp on a Loopback device + optional virtio-net NIC.
//! Loopback is always 127.0.0.1/8. The Ethernet interface (when present)
//! acquires its address via DHCPv4.

pub mod loopback;
pub mod sockets;
pub mod virtio;

use spin::Mutex;
use smoltcp::iface::{Config, Interface, SocketSet, SocketHandle};
use smoltcp::socket::dhcpv4;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr};

pub struct NetState {
    pub iface_lo:  Interface,
    pub dev_lo:    loopback::Loopback,
    pub iface_net: Option<Interface>,
    pub dev_net:   Option<virtio::VirtioNet>,
    pub sockets:   SocketSet<'static>,
    pub dhcp:      Option<SocketHandle>,
    dhcp_bound:    bool,
}

pub static NET: Mutex<Option<NetState>> = Mutex::new(None);

#[inline]
fn now() -> Instant {
    Instant::from_millis(crate::timer::ticks() as i64 * 10)
}

pub fn init() {
    let mut dev_lo = loopback::new();
    let mut iface_lo = Interface::new(Config::new(HardwareAddress::Ip), &mut dev_lo, now());
    iface_lo.update_ip_addrs(|a| {
        a.push(IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8)).unwrap();
    });

    let mut sockets = SocketSet::new(alloc::vec::Vec::new());

    let (iface_net, dev_net, dhcp) = match virtio::VirtioNet::find_and_init() {
        Some(mut nic) => {
            let mac = nic.mac();
            let cfg = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
            let iface = Interface::new(cfg, &mut nic, now());
            let handle = sockets.add(dhcpv4::Socket::new());
            (Some(iface), Some(nic), Some(handle))
        }
        None => {
            crate::bwarn!("net", "no virtio-net found — loopback only");
            (None, None, None)
        }
    };

    *NET.lock() = Some(NetState {
        iface_lo,
        dev_lo,
        iface_net,
        dev_net,
        sockets,
        dhcp,
        dhcp_bound: false,
    });
}

/// Called periodically by `net_poll_task` (every 10 ms).
pub fn poll() {
    use x86_64::instructions::interrupts::without_interrupts;
    without_interrupts(|| {
        let mut g = NET.lock();
        let Some(net) = g.as_mut() else { return; };
        let t = now();

        // Always poll loopback.
        let _ = net.iface_lo.poll(t, &mut net.dev_lo, &mut net.sockets);

        // Poll the Ethernet interface if present, then process DHCP events.
        if let (Some(iface), Some(dev)) = (net.iface_net.as_mut(), net.dev_net.as_mut()) {
            let _ = iface.poll(t, dev, &mut net.sockets);
        }

        // Process DHCP events: extract the event (releasing the socket borrow)
        // before touching iface_net.
        if let Some(h) = net.dhcp {
            let event = net.sockets.get_mut::<dhcpv4::Socket>(h).poll();
            // Copy the fields we need out of the event before we drop it —
            // Config::address (Ipv4Cidr) and Config::router (Option<Ipv4Address>)
            // are both Copy, but Config itself borrows the receive buffer, so we
            // extract the values into owned locals here.
            let action: Option<(Option<smoltcp::wire::Ipv4Cidr>, Option<smoltcp::wire::Ipv4Address>)> =
                match event {
                    Some(dhcpv4::Event::Configured(ref cfg)) => {
                        Some((Some(cfg.address), cfg.router))
                    }
                    Some(dhcpv4::Event::Deconfigured) => Some((None, None)),
                    None => None,
                };

            if let (Some(action), Some(iface)) = (action, net.iface_net.as_mut()) {
                match action {
                    (Some(addr), router) => {
                        iface.update_ip_addrs(|a| {
                            a.clear();
                            a.push(IpCidr::Ipv4(addr)).unwrap();
                        });
                        if let Some(gw) = router {
                            let _ = iface.routes_mut().add_default_ipv4_route(gw);
                        }
                        if !net.dhcp_bound {
                            net.dhcp_bound = true;
                            match router {
                                Some(gw) => crate::binfo!("net", "dhcp bound ip={} gw={}", addr.address(), gw),
                                None      => crate::binfo!("net", "dhcp bound ip={} gw=none", addr.address()),
                            }
                        }
                    }
                    (None, _) => {
                        iface.update_ip_addrs(|a| a.clear());
                        let _ = iface.routes_mut().remove_default_ipv4_route();
                        net.dhcp_bound = false;
                    }
                }
            }
        }
    });
}
