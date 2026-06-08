//! Netconsole — UDP broadcast log sink (compile-time `--features netconsole`).
//!
//! `emit()` (boot/log.rs) calls `enqueue()` for every log line. `enqueue` ONLY
//! pushes into `NC_RING` — it never locks `NET`, because `emit()` itself runs
//! inside the net poll task (the NIC driver logs while holding `NET`); a
//! synchronous UDP send from `emit()` would re-lock `NET` and deadlock.
//!
//! `net::poll()` (which already holds `NET`) calls `on_poll()` once per tick to
//! drain `NC_RING` into a UDP socket bound on the Ethernet `SocketSet`; the
//! ethernet `iface.poll` right after transmits it as a broadcast datagram.
//!
//! Nothing in the drain/send path logs (`binfo!`/`bwarn!`) — that would re-enter
//! `enqueue` and feed back on itself.

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use smoltcp::iface::{SocketHandle, SocketSet};
use smoltcp::socket::udp;
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

/// UDP port for both the local bind and the broadcast destination.
const PORT: u16 = 6666;
/// Ring capacity — holds the 32 KiB klog backlog plus live burst headroom.
const RING_CAP: usize = 48 * 1024;
/// Max datagram payload per send (kept well under a 1500-byte MTU).
const CHUNK: usize = 512;
/// Max datagrams flushed per poll tick (≈4 KiB/tick @ 100 Hz = ~400 KiB/s).
const MAX_PER_TICK: usize = 8;

/// True once DHCP has bound an address; before that `enqueue` is a no-op
/// (pre-bind lines live in klog and are recovered by the backlog flush).
static BOUND: AtomicBool = AtomicBool::new(false);
/// Handle of the UDP socket inside the Ethernet `SocketSet`.
static NC_HANDLE: Mutex<Option<SocketHandle>> = Mutex::new(None);
/// The byte queue. Producer = `enqueue`; consumer = `on_poll`.
static NC_RING: Mutex<NcRing> = Mutex::new(NcRing::new());

/// Bounded byte ring. On overflow the oldest bytes are dropped.
struct NcRing {
    buf:  [u8; RING_CAP],
    tail: usize, // oldest byte
    len:  usize, // bytes currently queued
}

impl NcRing {
    const fn new() -> Self {
        Self { buf: [0; RING_CAP], tail: 0, len: 0 }
    }

    fn push(&mut self, bytes: &[u8]) {
        for &b in bytes {
            let head = (self.tail + self.len) % RING_CAP;
            self.buf[head] = b;
            if self.len == RING_CAP {
                // Full: overwrite oldest, advance tail.
                self.tail = (self.tail + 1) % RING_CAP;
            } else {
                self.len += 1;
            }
        }
    }

    /// Copy up to `out.len()` queued bytes (oldest first) without consuming.
    fn peek(&self, out: &mut [u8]) -> usize {
        let n = self.len.min(out.len());
        for i in 0..n {
            out[i] = self.buf[(self.tail + i) % RING_CAP];
        }
        n
    }

    /// Drop `n` oldest bytes.
    fn advance(&mut self, n: usize) {
        let n = n.min(self.len);
        self.tail = (self.tail + n) % RING_CAP;
        self.len -= n;
    }
}

/// Create + bind the UDP socket on the Ethernet socket set. Called from
/// `net::init` after `net_sockets` is built.
pub fn init(net_sockets: &mut SocketSet<'static>) {
    let rx_meta = alloc::vec![udp::PacketMetadata::EMPTY; 4];
    let rx_buf  = alloc::vec![0u8; 256];
    let tx_meta = alloc::vec![udp::PacketMetadata::EMPTY; 32];
    let tx_buf  = alloc::vec![0u8; 8192];
    let mut sock = udp::Socket::new(
        udp::PacketBuffer::new(rx_meta, rx_buf),
        udp::PacketBuffer::new(tx_meta, tx_buf),
    );
    if sock.bind(PORT).is_err() {
        return; // leave NC_HANDLE None → on_poll stays a no-op
    }
    let handle = net_sockets.add(sock);
    *NC_HANDLE.lock() = Some(handle);
}

/// Queue a log line. No-op until DHCP-bound. Never touches `NET`.
pub fn enqueue(bytes: &[u8]) {
    if !BOUND.load(Ordering::Relaxed) {
        return;
    }
    NC_RING.lock().push(bytes);
}

/// Mark the interface bound and push the klog backlog so logs from T+0 reach
/// the listener. Idempotent. Called from `net::poll` on the DHCP bind edge,
/// while `NET` is held.
pub fn mark_bound() {
    if BOUND.swap(true, Ordering::Relaxed) {
        return; // already bound
    }
    let mut backlog = alloc::vec![0u8; 32 * 1024];
    let n = crate::klog::read(&mut backlog);
    NC_RING.lock().push(&backlog[..n]);
}

/// Drain the ring into the UDP socket. Called from `net::poll` (holds `NET`)
/// BEFORE the ethernet `iface.poll`, so the same poll transmits. Must not log.
pub fn on_poll(net_sockets: &mut SocketSet<'static>) {
    if !BOUND.load(Ordering::Relaxed) {
        return;
    }
    let handle = match *NC_HANDLE.lock() {
        Some(h) => h,
        None => return,
    };
    let sock = net_sockets.get_mut::<udp::Socket>(handle);
    let ep = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::BROADCAST), PORT);

    let mut ring = NC_RING.lock();
    let mut tmp = [0u8; CHUNK];
    for _ in 0..MAX_PER_TICK {
        let n = ring.peek(&mut tmp);
        if n == 0 {
            break;
        }
        // Prefer a datagram that ends on a line boundary for readable output.
        let cut = tmp[..n].iter().rposition(|&b| b == b'\n').map(|i| i + 1).unwrap_or(n);
        let cut = cut.max(1);
        match sock.send_slice(&tmp[..cut], ep) {
            Ok(()) => ring.advance(cut),
            Err(_) => break, // tx buffer full → retry next tick, keep bytes queued
        }
    }
}
