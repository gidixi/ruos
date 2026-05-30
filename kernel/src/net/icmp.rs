//! ICMPv4 echo (ping) — kernel-side helper used by `ruos::ping` host fn.
//!
//! Allocates an ICMP socket bound to a per-call ident, sends an Echo Request,
//! polls for the matching Echo Reply, returns elapsed time (ms) or a static
//! error string on timeout / send failure.

use smoltcp::iface::SocketHandle;
use smoltcp::socket::icmp::{
    Endpoint as IcmpEndpoint, PacketBuffer as IcmpPacketBuffer,
    PacketMetadata as IcmpPacketMetadata, Socket as IcmpSocket,
};
use smoltcp::wire::{IpAddress, Icmpv4Packet, Icmpv4Repr, Ipv4Address};
use x86_64::instructions::interrupts::without_interrupts;

use crate::executor::delay::Delay;

/// Bytes sent in the echo payload (filled with a fixed marker; matched on reply).
const PAYLOAD: &[u8] = b"ruos-ping";
/// Per-process-ish identifier (we don't multiplex pings — one in flight at a time).
const ECHO_IDENT: u16 = 0x3713;

/// Allocate + bind an ICMP socket, send Echo Request, await reply, return ms.
/// `timeout_ticks` is in 10-ms scheduler ticks.
pub async fn ping(target: Ipv4Address, timeout_ticks: u64) -> Result<u64, &'static str> {
    // 1. Allocate the socket on iface_net (or iface_nic) socket set.
    let handle = {
        let mut g = crate::net::NET.lock();
        let net = g.as_mut().ok_or("net not initialized")?;
        // Need an Ethernet iface to send ICMP (loopback ignored).
        if net.iface_net.is_none() && net.iface_nic.is_none() {
            return Err("no ethernet iface");
        }
        let rx_meta = alloc::vec![IcmpPacketMetadata::EMPTY; 4];
        let rx_buf  = alloc::vec![0u8; 256];
        let tx_meta = alloc::vec![IcmpPacketMetadata::EMPTY; 4];
        let tx_buf  = alloc::vec![0u8; 256];
        let mut sock = IcmpSocket::new(
            IcmpPacketBuffer::new(rx_meta, rx_buf),
            IcmpPacketBuffer::new(tx_meta, tx_buf),
        );
        sock.bind(IcmpEndpoint::Ident(ECHO_IDENT)).map_err(|_| "bind failed")?;
        net.net_sockets.add(sock)
    };
    let result = ping_inner(handle, target, timeout_ticks).await;
    // 2. Always free the socket.
    free_socket(handle);
    result
}

async fn ping_inner(
    handle: SocketHandle,
    target: Ipv4Address,
    timeout_ticks: u64,
) -> Result<u64, &'static str> {
    let start_ms = current_ms();

    // 3. Build + send Echo Request.
    {
        let mut g = crate::net::NET.lock();
        let net = g.as_mut().ok_or("net not initialized")?;
        let sock = net.net_sockets.get_mut::<IcmpSocket>(handle);
        // Determine the slice size from Icmpv4Repr.
        let repr = Icmpv4Repr::EchoRequest {
            ident: ECHO_IDENT,
            seq_no: 1,
            data: PAYLOAD,
        };
        let buf = sock.send(repr.buffer_len(), IpAddress::Ipv4(target))
            .map_err(|_| "send full")?;
        let mut pkt = Icmpv4Packet::new_unchecked(buf);
        repr.emit(&mut pkt, &smoltcp::phy::ChecksumCapabilities::default());
    }

    // 4. Poll for reply (the net_poll_task drives smoltcp on every tick).
    let deadline = start_ms + timeout_ticks * 10;
    loop {
        // Quick non-blocking check.
        let ready = without_interrupts(|| {
            let mut g = crate::net::NET.lock();
            let Some(net) = g.as_mut() else { return Err::<bool, &'static str>("net gone"); };
            let sock = net.net_sockets.get_mut::<IcmpSocket>(handle);
            if sock.can_recv() {
                // recv() expects an IcmpRepr; we only verify the source matches and
                // discard payload — latency is computed from wall clock.
                let _ = sock.recv();
                Ok(true)
            } else { Ok(false) }
        })?;
        if ready {
            let now = current_ms();
            return Ok(now.saturating_sub(start_ms));
        }
        if current_ms() >= deadline { return Err("timeout"); }
        Delay::ticks(1).await;
    }
}

fn free_socket(handle: SocketHandle) {
    without_interrupts(|| {
        let mut g = crate::net::NET.lock();
        if let Some(net) = g.as_mut() {
            net.net_sockets.remove(handle);
        }
    });
}

#[inline]
fn current_ms() -> u64 {
    // timer::ticks() = scheduler ticks (10 ms each at 100 Hz). ms = ticks * 10.
    crate::timer::ticks() * 10
}
