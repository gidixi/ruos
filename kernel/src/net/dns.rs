//! DNS resolver — kernel-side helper used by `ruos::net_resolve` host fn.
//!
//! Drives the smoltcp DNS socket allocated at `net::init()` (servers come from
//! the DHCP lease). Issues an A query, then polls for completion on scheduler
//! ticks (the net poll task advances the socket), like `icmp::ping`.

use alloc::vec::Vec;
use smoltcp::socket::dns;
use smoltcp::wire::{DnsQueryType, IpAddress, Ipv4Address};
use x86_64::instructions::interrupts::without_interrupts;

use crate::executor::delay::Delay;

/// Safety deadline: smoltcp gives up retransmitting after ~10 s and marks the
/// query Failed; this only guards against the socket never being driven.
const TIMEOUT_TICKS: u64 = 1500; // 15 s @ 100 Hz

#[derive(Debug)]
pub enum DnsError {
    NoServers,
    Failed,
    Timeout,
}

pub async fn resolve(name: &str) -> Result<Vec<Ipv4Address>, DnsError> {
    let handle = {
        let mut g = crate::net::NET.lock();
        let net = match g.as_mut() {
            Some(n) => n,
            None => return Err(DnsError::Failed),
        };
        if net.dns_servers.is_empty() {
            return Err(DnsError::NoServers);
        }
        let (dns_h, iface) = match (net.dns, net.iface_net.as_mut().or_else(|| net.iface_nic.as_mut()).or_else(|| net.iface_wifi.as_mut())) {
            (Some(h), Some(i)) => (h, i),
            _ => return Err(DnsError::Failed),
        };
        let socket = net.net_sockets.get_mut::<dns::Socket>(dns_h);
        match socket.start_query(iface.context(), name, DnsQueryType::A) {
            Ok(handle) => handle,
            Err(e) => {
                crate::bwarn!("dns", "start_query error: {:?}", e);
                return Err(DnsError::Failed);
            }
        }
    };

    // Poll for the result on each tick (the net poll task drives smoltcp).
    let deadline = crate::timer::ticks() + TIMEOUT_TICKS;
    loop {
        let done = without_interrupts(|| {
            let mut g = crate::net::NET.lock();
            let net = match g.as_mut() {
                Some(n) => n,
                None => return Some(Err(DnsError::Failed)),
            };
            let dns_h = match net.dns {
                Some(h) => h,
                None => return Some(Err(DnsError::Failed)),
            };
            let socket = net.net_sockets.get_mut::<dns::Socket>(dns_h);
            match socket.get_query_result(handle) {
                Ok(addrs) => {
                    let v4: Vec<Ipv4Address> = addrs.iter()
                        .filter_map(|a| match a {
                            IpAddress::Ipv4(v) => Some(*v),
                        })
                        .collect();
                    Some(Ok(v4))
                }
                Err(dns::GetQueryResultError::Pending) => None,
                Err(e) => {
                    crate::bwarn!("dns", "query result error: {:?}", e);
                    Some(Err(DnsError::Failed))
                }
            }
        });
        if let Some(result) = done {
            return result;
        }
        if crate::timer::ticks() >= deadline {
            // Free the slot so a stuck query doesn't leak it.
            without_interrupts(|| {
                let mut g = crate::net::NET.lock();
                if let Some(net) = g.as_mut() {
                    if let Some(dns_h) = net.dns {
                        net.net_sockets.get_mut::<dns::Socket>(dns_h).cancel_query(handle);
                    }
                }
            });
            return Err(DnsError::Timeout);
        }
        Delay::ticks(1).await;
    }
}
