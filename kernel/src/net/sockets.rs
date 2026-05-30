//! Kernel-side socket pool. Each wasm-side FD that is a socket maps
//! to a smoltcp SocketHandle here. The wasm host fns manipulate this
//! pool; the net_poll_task drives smoltcp via net::poll() so async
//! wrappers just yield via Delay::ticks(1) and re-check.

use alloc::vec::Vec;
use spin::Mutex;
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer};
use smoltcp::wire::{IpAddress, IpEndpoint, IpListenEndpoint};
use x86_64::instructions::interrupts::without_interrupts;

const BUF_SIZE: usize = 4096;

/// Dispatch by `POOL.is_ethernet(handle)`: route to the matching SocketSet.
#[inline]
fn tcp_get_mut<'a>(net: &'a mut crate::net::NetState, h: SocketHandle)
    -> &'a mut TcpSocket<'static>
{
    if POOL.is_ethernet(h) { net.net_sockets.get_mut::<TcpSocket>(h) }
    else                   { net.sockets.get_mut::<TcpSocket>(h) }
}
#[inline]
fn tcp_get<'a>(net: &'a crate::net::NetState, h: SocketHandle)
    -> &'a TcpSocket<'static>
{
    if POOL.is_ethernet(h) { net.net_sockets.get::<TcpSocket>(h) }
    else                   { net.sockets.get::<TcpSocket>(h) }
}

pub struct SockEntry {
    pub handle: SocketHandle,
    /// `true` if the socket lives in `net.net_sockets` (Ethernet),
    /// `false` if in `net.sockets` (loopback).
    pub ethernet: bool,
}

pub struct SockPool {
    inner: Mutex<Vec<Option<SockEntry>>>,
}

pub static POOL: SockPool = SockPool {
    inner: Mutex::new(Vec::new()),
};

impl SockPool {
    /// Allocate a new TCP socket on the loopback SocketSet.
    pub fn alloc_tcp(&self) -> usize { self.alloc_tcp_in(false) }
    /// Allocate a new TCP socket on the Ethernet SocketSet. Use for any
    /// listener that must receive traffic from outside the kernel (SSH).
    pub fn alloc_tcp_eth(&self) -> usize { self.alloc_tcp_in(true) }

    fn alloc_tcp_in(&self, ethernet: bool) -> usize {
        without_interrupts(|| {
            let rx = SocketBuffer::new(alloc::vec![0u8; BUF_SIZE]);
            let tx = SocketBuffer::new(alloc::vec![0u8; BUF_SIZE]);
            let socket = TcpSocket::new(rx, tx);
            let mut g = crate::net::NET.lock();
            let net = g.as_mut().expect("net not initialized");
            let handle = if ethernet {
                net.net_sockets.add(socket)
            } else {
                net.sockets.add(socket)
            };
            drop(g);
            let mut inner = self.inner.lock();
            let entry = SockEntry { handle, ethernet };
            for (i, slot) in inner.iter_mut().enumerate() {
                if slot.is_none() {
                    *slot = Some(entry);
                    return i;
                }
            }
            inner.push(Some(entry));
            inner.len() - 1
        })
    }

    /// `true` if pool entry at `idx` lives in the Ethernet SocketSet.
    pub fn is_ethernet(&self, h: SocketHandle) -> bool {
        let g = self.inner.lock();
        g.iter().flatten().any(|e| e.handle == h && e.ethernet)
    }

    pub fn handle(&self, idx: usize) -> Option<SocketHandle> {
        let g = self.inner.lock();
        g.get(idx).and_then(|x| x.as_ref()).map(|e| e.handle)
    }
}

/// Put a socket into listening state on the given port.
pub fn listen(handle: SocketHandle, port: u16) -> Result<(), &'static str> {
    without_interrupts(|| {
        let mut g = crate::net::NET.lock();
        let net = g.as_mut().expect("net not initialized");
        let s = tcp_get_mut(net, handle);
        s.listen(port).map_err(|_| "listen failed")
    })
}

/// Async connect: initiate TCP connection then yield until Established.
pub async fn connect(
    handle: SocketHandle,
    remote: IpEndpoint,
    local_port: u16,
) -> Result<(), &'static str> {
    without_interrupts(|| {
        let mut g = crate::net::NET.lock();
        let net = g.as_mut().expect("net not initialized");
        // Use the interface owning the socket: loopback for in-kernel,
        // Ethernet for outbound. (The Ethernet iface_net / iface_nic
        // pair are one or the other; pick whichever exists.)
        let eth = POOL.is_ethernet(handle);
        let local: IpListenEndpoint = local_port.into();
        if eth {
            let iface = net.iface_net.as_mut().or_else(|| net.iface_nic.as_mut())
                .expect("connect: no ethernet iface");
            let ctx = iface.context();
            let s = net.net_sockets.get_mut::<TcpSocket>(handle);
            s.connect(ctx, remote, local).map_err(|_| "connect failed")
        } else {
            let ctx = net.iface_lo.context();
            let s = net.sockets.get_mut::<TcpSocket>(handle);
            s.connect(ctx, remote, local).map_err(|_| "connect failed")
        }
    })?;
    // Yield until Established (net_poll_task drives smoltcp between yields).
    loop {
        let done = without_interrupts(|| {
            let g = crate::net::NET.lock();
            let net = g.as_ref().expect("net not initialized");
            let s = tcp_get(net, handle);
            use smoltcp::socket::tcp::State;
            match s.state() {
                State::Established => Some(Ok(())),
                State::Closed | State::TimeWait | State::CloseWait => {
                    Some(Err("connect: socket closed"))
                }
                _ => None,
            }
        });
        if let Some(result) = done {
            return result;
        }
        crate::executor::delay::Delay::ticks(1).await;
    }
}

/// Async accept: yield until the listening socket transitions to Established.
pub async fn accept(handle: SocketHandle) -> Result<(), &'static str> {
    use smoltcp::socket::tcp::State;
    loop {
        let ready = without_interrupts(|| {
            let g = crate::net::NET.lock();
            let net = g.as_ref().expect("net not initialized");
            tcp_get(net, handle).state() == State::Established
        });
        if ready {
            return Ok(());
        }
        crate::executor::delay::Delay::ticks(1).await;
    }
}

/// Non-blocking recv: return Some(n) if data was available, None otherwise.
/// `n == 0` indicates peer closed (half-close on the read side).
pub fn try_recv(handle: SocketHandle, buf: &mut [u8]) -> Option<usize> {
    use smoltcp::socket::tcp::State;
    without_interrupts(|| {
        let mut g = crate::net::NET.lock();
        let net = g.as_mut().expect("net not initialized");
        let s = tcp_get_mut(net, handle);
        // Only signal close-of-input when the socket really moved to a closed
        // state. While Established, recv_slice returning Ok(0) just means
        // "no data right now" — surface that as None to the caller.
        if matches!(s.state(), State::CloseWait | State::Closed | State::TimeWait) {
            return Some(0);
        }
        if !s.can_recv() {
            return None;
        }
        match s.recv_slice(buf) {
            Ok(0)  => None,
            Ok(n)  => Some(n),
            Err(_) => None,
        }
    })
}

/// Non-blocking send: return Some(n) if any bytes were written, None
/// otherwise. `n == 0` means socket closed for write.
pub fn try_send(handle: SocketHandle, buf: &[u8]) -> Option<usize> {
    use smoltcp::socket::tcp::State;
    without_interrupts(|| {
        let mut g = crate::net::NET.lock();
        let net = g.as_mut().expect("net not initialized");
        let s = tcp_get_mut(net, handle);
        if s.can_send() {
            s.send_slice(buf).ok()
        } else if matches!(s.state(), State::Closed | State::TimeWait) {
            Some(0)
        } else {
            None
        }
    })
}

/// Close the socket (both halves).
pub fn close(handle: SocketHandle) {
    without_interrupts(|| {
        let mut g = crate::net::NET.lock();
        let net = g.as_mut().expect("net not initialized");
        let s = tcp_get_mut(net, handle);
        s.close();
    });
}

/// Async recv: yield until data is available, return bytes read.
pub async fn recv(handle: SocketHandle, buf: &mut [u8]) -> Result<usize, &'static str> {
    loop {
        let n = without_interrupts(|| {
            let mut g = crate::net::NET.lock();
            let net = g.as_mut().expect("net not initialized");
            let s = tcp_get_mut(net, handle);
            if s.can_recv() {
                s.recv_slice(buf).ok()
            } else {
                None
            }
        });
        if let Some(n) = n {
            if n > 0 {
                return Ok(n);
            }
        }
        crate::executor::delay::Delay::ticks(1).await;
    }
}

/// Async send: yield until the socket TX buffer has room, then write.
pub async fn send(handle: SocketHandle, buf: &[u8]) -> Result<usize, &'static str> {
    loop {
        let n = without_interrupts(|| {
            let mut g = crate::net::NET.lock();
            let net = g.as_mut().expect("net not initialized");
            let s = tcp_get_mut(net, handle);
            if s.can_send() {
                s.send_slice(buf).ok()
            } else {
                None
            }
        });
        if let Some(n) = n {
            if n > 0 {
                return Ok(n);
            }
        }
        crate::executor::delay::Delay::ticks(1).await;
    }
}
