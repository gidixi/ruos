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

pub struct SockEntry {
    pub handle: SocketHandle,
}

pub struct SockPool {
    inner: Mutex<Vec<Option<SockEntry>>>,
}

pub static POOL: SockPool = SockPool {
    inner: Mutex::new(Vec::new()),
};

impl SockPool {
    /// Allocate a new TCP socket in the smoltcp SocketSet and return
    /// its pool index.
    pub fn alloc_tcp(&self) -> usize {
        without_interrupts(|| {
            let rx = SocketBuffer::new(alloc::vec![0u8; BUF_SIZE]);
            let tx = SocketBuffer::new(alloc::vec![0u8; BUF_SIZE]);
            let socket = TcpSocket::new(rx, tx);
            let mut g = crate::net::NET.lock();
            let net = g.as_mut().expect("net not initialized");
            let handle = net.sockets.add(socket);
            drop(g);
            let mut inner = self.inner.lock();
            let entry = SockEntry { handle };
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
        let s = net.sockets.get_mut::<TcpSocket>(handle);
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
        let ctx = net.iface_lo.context();
        let s = net.sockets.get_mut::<TcpSocket>(handle);
        let local: IpListenEndpoint = local_port.into();
        s.connect(ctx, remote, local).map_err(|_| "connect failed")
    })?;
    // Yield until Established (net_poll_task drives smoltcp between yields).
    loop {
        let done = without_interrupts(|| {
            let g = crate::net::NET.lock();
            let net = g.as_ref().expect("net not initialized");
            let s = net.sockets.get::<TcpSocket>(handle);
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
            net.sockets.get::<TcpSocket>(handle).state() == State::Established
        });
        if ready {
            return Ok(());
        }
        crate::executor::delay::Delay::ticks(1).await;
    }
}

/// Async recv: yield until data is available, return bytes read.
pub async fn recv(handle: SocketHandle, buf: &mut [u8]) -> Result<usize, &'static str> {
    loop {
        let n = without_interrupts(|| {
            let mut g = crate::net::NET.lock();
            let net = g.as_mut().expect("net not initialized");
            let s = net.sockets.get_mut::<TcpSocket>(handle);
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
            let s = net.sockets.get_mut::<TcpSocket>(handle);
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
