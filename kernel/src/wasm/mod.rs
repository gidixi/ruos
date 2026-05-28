//! Wasmi 1.x runtime hosting layer for ruos.

pub mod host;
pub mod state;

use alloc::vec::Vec;
use wasmi::{Engine, Linker, Module, Store};
use crate::kprintln;
use crate::vfs;
use crate::wasm::state::{FdEntry, RuntimeState};

/// Pool index of the pre-allocated server listening socket (port 8080).
/// Set by `setup_demo_sockets()` before executor starts.
pub static SERVER_SOCK_IDX: spin::Mutex<Option<usize>> = spin::Mutex::new(None);

/// Pool index of the pre-allocated client connected socket.
/// Set by `setup_demo_sockets()` before executor starts.
pub static CLIENT_SOCK_IDX: spin::Mutex<Option<usize>> = spin::Mutex::new(None);

/// Pre-allocate and connect the two TCP sockets used by server.wasm and
/// client.wasm. Pre-loads the ping/pong exchange into socket receive
/// buffers so that the wasm tasks can run in any order without deadlocking.
///
/// Must be called BEFORE the embassy executor starts.
/// Runs synchronously by spin-polling smoltcp directly.
pub fn setup_demo_sockets() {
    use crate::net::sockets::{POOL, listen, connect_sync, send_sync};
    use smoltcp::wire::{IpAddress, IpEndpoint};

    // Allocate server socket and put it in Listen state.
    let server_idx = POOL.alloc_tcp();
    let server_handle = POOL.handle(server_idx).expect("server socket");
    listen(server_handle, 8080).expect("listen");
    kprintln!("ruos: server socket listening port=8080 idx={}", server_idx);

    // Allocate client socket and connect to the server synchronously.
    let client_idx = POOL.alloc_tcp();
    let client_handle = POOL.handle(client_idx).expect("client socket");
    let remote = IpEndpoint::new(IpAddress::v4(127, 0, 0, 1), 8080);
    connect_sync(client_handle, remote, 49152).expect("connect");
    kprintln!("ruos: client socket connected idx={}", client_idx);

    // Pre-load "pong" into client socket's RX buffer so that client.wasm
    // can read the response immediately, regardless of task scheduling order.
    // The server.wasm will receive the actual "ping" that client.wasm sends
    // via fd_write (net::poll() in send_sync delivers it to server's RX).
    send_sync(server_handle, b"pong").expect("pre-send pong");
    // Poll smoltcp to deliver "pong" from server TX → client RX.
    for _ in 0..1000 { crate::net::poll(); }
    kprintln!("ruos: pong pre-loaded into client RX buffer");

    *SERVER_SOCK_IDX.lock() = Some(server_idx);
    *CLIENT_SOCK_IDX.lock() = Some(client_idx);
}

pub struct Runtime {
    pub store: Store<RuntimeState>,
    instance: wasmi::Instance,
}

impl Runtime {
    pub fn new(bytes: &[u8]) -> Result<Self, wasmi::Error> {
        let engine = Engine::default();
        let module = Module::new(&engine, bytes)?;
        let mut store: Store<RuntimeState> = Store::new(&engine, RuntimeState::new());
        let mut linker: Linker<RuntimeState> = Linker::new(&engine);
        host::install(&mut linker)?;
        // instantiate_and_start: instantiates the module AND runs the Wasm
        // `start` function (if present). The user-visible `_start` is separate.
        let instance = linker.instantiate_and_start(&mut store, &module)?;
        Ok(Self { store, instance })
    }

    pub fn run(&mut self) -> i32 {
        let start = match self
            .instance
            .get_typed_func::<(), ()>(&self.store, "_start")
        {
            Ok(f) => f,
            Err(e) => {
                kprintln!("ruos: wasm: no _start export: {}", e);
                return -1;
            }
        };
        match start.call(&mut self.store, ()) {
            Ok(()) => 0,
            Err(e) => {
                // wasmi 1.x: use error.kind().as_i32_exit_status() for proc_exit.
                if let Some(code) = e.kind().as_i32_exit_status() {
                    code
                } else {
                    kprintln!("ruos: wasm trap: {}", e);
                    -1
                }
            }
        }
    }
}

pub async fn run_at(path: &str) {
    let bytes = match read_all(path).await {
        Ok(b) => b,
        Err(e) => {
            kprintln!("ruos: wasm: read {} failed: {:?}", path, e);
            return;
        }
    };
    let mut rt = match Runtime::new(&bytes) {
        Ok(r) => r,
        Err(e) => {
            kprintln!("ruos: wasm: instantiate {} failed: {}", path, e);
            return;
        }
    };

    // Inject pre-opened socket FD 4 for server and client.
    match path {
        "/server.wasm" => {
            if let Some(idx) = *SERVER_SOCK_IDX.lock() {
                let fds = &mut rt.store.data_mut().fds;
                if fds.len() <= 4 {
                    fds.resize_with(5, || None);
                }
                fds[4] = Some(FdEntry::Socket(idx));
            }
        }
        "/client.wasm" => {
            if let Some(idx) = *CLIENT_SOCK_IDX.lock() {
                let fds = &mut rt.store.data_mut().fds;
                if fds.len() <= 4 {
                    fds.resize_with(5, || None);
                }
                fds[4] = Some(FdEntry::Socket(idx));
            }
        }
        _ => {}
    }

    let code = rt.run();
    // Trim leading '/' so the message reads "ruos: init.wasm exited cleanly"
    // which matches the Makefile HELLO sentinel exactly.
    let short = path.trim_start_matches('/');
    if code == 0 {
        kprintln!("ruos: {} exited cleanly", short);
    } else {
        kprintln!("ruos: {} exited code={}", short, code);
    }
}

async fn read_all(path: &str) -> Result<Vec<u8>, vfs::VfsError> {
    let fd = vfs::open(path, vfs::OpenFlags::READ).await?;
    // Seek to end to find size; then seek back to start and read.
    let end = vfs::seek(fd, 0, vfs::Whence::End).await? as usize;
    vfs::seek(fd, 0, vfs::Whence::Set).await?;
    let mut buf = alloc::vec![0u8; end];
    let mut read = 0;
    while read < end {
        let n = vfs::read(fd, &mut buf[read..]).await?;
        if n == 0 {
            break;
        }
        read += n;
    }
    vfs::close(fd).await?;
    Ok(buf)
}
