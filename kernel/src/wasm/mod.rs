//! Wasmi 1.x runtime hosting layer for ruos.

pub mod host;
pub mod state;

use alloc::vec::Vec;
use wasmi::{Engine, Linker, Module, Store};
use crate::kprintln;
use crate::vfs;
use crate::wasm::state::RuntimeState;

pub struct Runtime {
    store: Store<RuntimeState>,
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
