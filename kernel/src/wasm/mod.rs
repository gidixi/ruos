//! Wasmi 1.x runtime hosting layer for ruos.

pub mod host;
pub mod state;
pub mod suspend;
pub mod fiber;

use alloc::vec::Vec;
use wasmi::{Engine, Linker, Module, Store};
use crate::kprintln;
use crate::vfs;
use crate::wasm::state::RuntimeState;

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

    kprintln!("ruos: wasm: about to instantiate {}", path);
    let mut fb = match crate::wasm::fiber::Fiber::new(&bytes) {
        Ok(f) => f,
        Err(e) => {
            kprintln!("ruos: wasm: instantiate {} failed: {}", path, e);
            return;
        }
    };
    kprintln!("ruos: wasm: instantiated {}", path);

    // Pre-open socket FD 4 for server and client.
    // Server: allocate + listen (sync instant); cooperative accept happens in fiber dispatch.
    // Client: allocate + async connect (yields until Established, then inject FD 4).
    match path {
        "/server.wasm" => {
            let idx = crate::net::sockets::POOL.alloc_tcp();
            let handle = crate::net::sockets::POOL.handle(idx).expect("server socket");
            crate::net::sockets::listen(handle, 8080).expect("listen");
            kprintln!("ruos: server socket listening port=8080 idx={}", idx);
            let fds = &mut fb.store.data_mut().fds;
            if fds.len() <= 4 {
                fds.resize_with(5, || None);
            }
            fds[4] = Some(crate::wasm::state::FdEntry::Socket(idx));
        }
        "/client.wasm" => {
            use smoltcp::wire::{IpAddress, IpEndpoint};
            let idx = crate::net::sockets::POOL.alloc_tcp();
            let handle = crate::net::sockets::POOL.handle(idx).expect("client socket");
            let remote = IpEndpoint::new(IpAddress::v4(127, 0, 0, 1), 8080);
            kprintln!("ruos: client socket connecting idx={}", idx);
            match crate::net::sockets::connect(handle, remote, 49152).await {
                Ok(()) => kprintln!("ruos: client socket connected idx={}", idx),
                Err(e) => {
                    kprintln!("ruos: client socket connect failed: {}", e);
                    return;
                }
            }
            let fds = &mut fb.store.data_mut().fds;
            if fds.len() <= 4 {
                fds.resize_with(5, || None);
            }
            fds[4] = Some(crate::wasm::state::FdEntry::Socket(idx));
        }
        _ => {}
    }

    let code = fb.run().await;
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
