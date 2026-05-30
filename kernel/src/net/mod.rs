//! Network stack — smoltcp on a Loopback device + optional virtio-net NIC.
//! Loopback is always 127.0.0.1/8. The Ethernet interface (when present)
//! acquires its address via DHCPv4.

pub mod icmp;
pub mod loopback;
pub mod nic;
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
    // Ethernet: at most ONE of (virtio | nic) is active. The matching iface_*
    // is also populated; the other pair stays None. Splitting per-family lets
    // smoltcp see a concrete device type at compile time (no enum dispatch).
    pub iface_net: Option<Interface>,
    pub dev_net:   Option<virtio::VirtioNet>,
    pub iface_nic: Option<Interface>,
    pub dev_nic:   Option<nic::Nic>,
    // App TCP sockets currently route through iface_lo only (sockets::connect
    // uses iface_lo.context()), i.e. loopback. Wiring app sockets over the
    // Ethernet iface is a Step 16 (SSH) item.
    pub sockets:   SocketSet<'static>,      // app/loopback sockets (iface_lo)
    pub net_sockets: SocketSet<'static>,    // Ethernet sockets incl. DHCP
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

    let sockets = SocketSet::new(alloc::vec::Vec::new());
    // Separate SocketSet for the Ethernet interface. The DHCPv4 socket MUST NOT
    // share a set with the loopback interface: smoltcp panics if a dhcpv4 socket
    // is polled by a non-Ethernet (Ip-medium) interface, and iface_lo would hit
    // it on every poll. Keeping it here means only iface_net ever services it.
    let mut net_sockets = SocketSet::new(alloc::vec::Vec::new());

    // Prefer virtio-net (paravirtual, fast in VMs). Fall back to the first
    // real-hardware NIC the `nic` probe table recognises (e1000 in MVP).
    let mut iface_net: Option<Interface>      = None;
    let mut dev_net:   Option<virtio::VirtioNet> = None;
    let mut iface_nic: Option<Interface>      = None;
    let mut dev_nic:   Option<nic::Nic>       = None;
    let mut dhcp: Option<SocketHandle>        = None;

    if let Some(mut d) = virtio::VirtioNet::find_and_init() {
        let mac = d.mac();
        let cfg = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
        iface_net = Some(Interface::new(cfg, &mut d, now()));
        dev_net   = Some(d);
        dhcp      = Some(net_sockets.add(dhcpv4::Socket::new()));
    } else if let Some(mut d) = nic::probe_and_init() {
        let mac = d.mac();
        let cfg = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
        iface_nic = Some(Interface::new(cfg, &mut d, now()));
        dev_nic   = Some(d);
        dhcp      = Some(net_sockets.add(dhcpv4::Socket::new()));
    } else {
        crate::bwarn!("net", "no Ethernet NIC found — loopback only");
    }

    *NET.lock() = Some(NetState {
        iface_lo,
        dev_lo,
        iface_net,
        dev_net,
        iface_nic,
        dev_nic,
        sockets,
        net_sockets,
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

        // Poll whichever Ethernet interface is active (virtio xor nic).
        if let (Some(iface), Some(dev)) = (net.iface_net.as_mut(), net.dev_net.as_mut()) {
            let _ = iface.poll(t, dev, &mut net.net_sockets);
        }
        if let (Some(iface), Some(dev)) = (net.iface_nic.as_mut(), net.dev_nic.as_mut()) {
            let _ = iface.poll(t, dev, &mut net.net_sockets);
        }

        // Process DHCP events: extract the event (releasing the socket borrow)
        // before touching whichever iface owns the DHCP lease (virtio xor nic).
        if let Some(h) = net.dhcp {
            let event = net.net_sockets.get_mut::<dhcpv4::Socket>(h).poll();
            let action: Option<(Option<smoltcp::wire::Ipv4Cidr>, Option<smoltcp::wire::Ipv4Address>)> =
                match event {
                    Some(dhcpv4::Event::Configured(ref cfg)) => {
                        Some((Some(cfg.address), cfg.router))
                    }
                    Some(dhcpv4::Event::Deconfigured) => Some((None, None)),
                    None => None,
                };

            // Apply to whichever iface was created at init.
            let iface = net.iface_net.as_mut().or_else(|| net.iface_nic.as_mut());
            if let (Some(action), Some(iface)) = (action, iface) {
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
