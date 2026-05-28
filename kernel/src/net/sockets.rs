//! Kernel-side socket pool. Each wasm-side FD that is a socket maps
//! to a smoltcp SocketHandle here. The wasm host fns manipulate this
//! pool; the underlying smoltcp Interface is driven inline (no separate
//! net_poll_task dependency — we call net::poll() directly in the
//! spin loops so this works from inside a sync host fn).

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

/// Connect a socket to a remote endpoint, then spin-poll until
/// the connection is established (or fails).
///
/// NOTE: calls net::poll() directly in the spin loop so this can
/// be called from synchronous (non-async) context.
pub fn connect_sync(
    handle: SocketHandle,
    remote: IpEndpoint,
    local_port: u16,
) -> Result<(), &'static str> {
    without_interrupts(|| {
        let mut g = crate::net::NET.lock();
        let net = g.as_mut().expect("net not initialized");
        let ctx = net.iface.context();
        let s = net.sockets.get_mut::<TcpSocket>(handle);
        let local: IpListenEndpoint = local_port.into();
        s.connect(ctx, remote, local).map_err(|_| "connect failed")
    })?;
    // Spin-poll until Established or an error state.
    for _ in 0..10_000 {
        crate::net::poll();
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
    }
    Err("connect: timed out")
}

/// Block until a client connects to the listening socket.
///
/// NOTE: calls net::poll() directly so it can be used from host fns.
pub fn accept_sync(handle: SocketHandle) -> Result<(), &'static str> {
    use smoltcp::socket::tcp::State;
    for _ in 0..100_000 {
        crate::net::poll();
        let ready = without_interrupts(|| {
            let g = crate::net::NET.lock();
            let net = g.as_ref().expect("net not initialized");
            net.sockets.get::<TcpSocket>(handle).state() == State::Established
        });
        if ready {
            return Ok(());
        }
    }
    Err("accept: timed out")
}

/// Synchronous receive: spin-poll until data is available, return
/// bytes read (may be less than buf.len()).
pub fn recv_sync(handle: SocketHandle, buf: &mut [u8]) -> Result<usize, &'static str> {
    for _ in 0..100_000 {
        crate::net::poll();
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
    }
    Err("recv: timed out")
}

/// Synchronous send: spin-poll until the data can be written.
pub fn send_sync(handle: SocketHandle, buf: &[u8]) -> Result<usize, &'static str> {
    for _ in 0..100_000 {
        crate::net::poll();
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
    }
    Err("send: timed out")
}
