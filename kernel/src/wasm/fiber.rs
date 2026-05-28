//! Cooperative wasm fiber driven by Func::call_resumable.
//!
//! `Fiber::run` is an async function.  When a host fn needs to wait, it
//! returns `Err(Error::host(SuspendReason::...))` which surfaces as a
//! `ResumableCall::HostTrap`.  `run` awaits the appropriate kernel
//! future, writes results into wasm memory, then calls `resume` so the
//! wasm function continues where it left off.
//!
//! # Compilation mode
//!
//! The Engine uses `CompilationMode::Eager` so that all WASM functions are
//! compiled during `Module::new` (synchronous, before the fiber starts).
//! Lazy compilation inside `execute_root_func` causes the operator-translation
//! loop for large functions to spin for many thousands of iterations, which
//! stalls the cooperative executor indefinitely on the first call.

use wasmi::{Engine, Module, Store, Linker, Instance, Val, ResumableCall};
use crate::kprintln;
use crate::wasm::state::RuntimeState;
use crate::wasm::host;
use crate::wasm::suspend::SuspendReason;

pub struct Fiber {
    pub store: Store<RuntimeState>,
    instance: Instance,
}

impl Fiber {
    pub fn new(bytes: &[u8]) -> Result<Self, wasmi::Error> {
        // Eager compilation: compile all WASM functions during Module::new so
        // that call_resumable never triggers lazy translation mid-execution.
        let mut config = wasmi::Config::default();
        config.compilation_mode(wasmi::CompilationMode::Eager);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, bytes)?;
        let mut store: Store<RuntimeState> = Store::new(&engine, RuntimeState::new());
        let mut linker: Linker<RuntimeState> = Linker::new(&engine);
        host::install(&mut linker)?;
        let instance = linker.instantiate_and_start(&mut store, &module)?;
        Ok(Self { store, instance })
    }

    pub async fn run(&mut self) -> i32 {
        // Get the _start function.
        let start = match self.instance.get_func(&self.store, "_start") {
            Some(f) => f,
            None => {
                kprintln!("ruos: wasm: no _start export");
                return -1;
            }
        };

        let mut outputs: [Val; 0] = [];
        let mut inv = match start.call_resumable(&mut self.store, &[], &mut outputs) {
            Ok(i) => i,
            Err(e) => { kprintln!("ruos: wasm: call_resumable error: {}", e); return Self::error_to_exit(&e); }
        };

        loop {
            match inv {
                ResumableCall::Finished => return 0,
                ResumableCall::HostTrap(state) => {
                    // Extract the SuspendReason from the host error.
                    // We clone the reason so we can release the borrow on `state`
                    // before calling `state.resume(...)`.
                    let maybe_reason: Option<SuspendReason> =
                        state.host_error().downcast_ref::<SuspendReason>().cloned();

                    match maybe_reason {
                        None => {
                            // Not a SuspendReason — might be a proc_exit or real trap.
                            let e = state.into_host_error();
                            return Self::error_to_exit(&e);
                        }
                        Some(reason) => {
                            kprintln!("ruos: wasm fiber: suspend {:?}", reason);
                            let errno = self.dispatch(reason).await;
                            let resume_args = [Val::I32(errno)];
                            let mut next_outputs: [Val; 0] = [];
                            inv = match state.resume(&mut self.store, &resume_args, &mut next_outputs) {
                                Ok(i) => i,
                                Err(e) => return Self::error_to_exit(&e),
                            };
                        }
                    }
                }
                ResumableCall::OutOfFuel(_) => {
                    kprintln!("ruos: wasm: out of fuel (unexpected — fuel not configured)");
                    return -1;
                }
            }
        }
    }

    async fn dispatch(&mut self, reason: SuspendReason) -> i32 {
        match reason {
            SuspendReason::Sleep { ticks, events_ptr, nevents_ptr } => {
                kprintln!("ruos: wasm fiber: sleeping {} ticks", ticks);
                crate::executor::delay::Delay::ticks(ticks).await;
                // Write one clock subscription_event (32 bytes) at events_ptr.
                // WASI wasi_event_t layout (32 bytes):
                //   userdata: u64 (0..8)  - leave 0
                //   error: u16 (8..10)    - 0 = ESUCCESS
                //   type: u8 (10)         - 0 = CLOCK
                //   padding to 32 bytes
                let event = [0u8; 32];
                let _ = self.write_to_memory(events_ptr, &event);
                let _ = self.write_u32(nevents_ptr, 1);
                kprintln!("ruos: wasm fiber: sleep done, writing 1 event");
                0
            }
            SuspendReason::SockAccept { handle, new_fd_ptr } => {
                kprintln!("ruos: wasm fiber: sock accept waiting");
                match crate::net::sockets::accept(handle).await {
                    Ok(()) => {
                        // smoltcp's listen socket transitions to Established;
                        // there's no separate new socket. Write current fd.
                        let cur_fd: u32 = self.find_fd_for_handle(handle).unwrap_or(0);
                        let _ = self.write_u32(new_fd_ptr, cur_fd);
                        kprintln!("ruos: wasm fiber: sock accepted fd={}", cur_fd);
                        0
                    }
                    Err(e) => {
                        kprintln!("ruos: wasm fiber: sock accept err: {}", e);
                        8
                    }
                }
            }
            SuspendReason::SockConnect { handle, remote, local_port } => {
                kprintln!("ruos: wasm fiber: sock connect to {:?}:{}", remote.addr, remote.port);
                match crate::net::sockets::connect(handle, remote, local_port).await {
                    Ok(()) => {
                        kprintln!("ruos: wasm fiber: sock connected");
                        0
                    }
                    Err(e) => {
                        kprintln!("ruos: wasm fiber: sock connect err: {}", e);
                        8
                    }
                }
            }
            SuspendReason::SockRecv { handle, buf_ptr, max_len, nrecv_ptr } => {
                kprintln!("ruos: wasm fiber: sock recv max={}", max_len);
                let mut buf = alloc::vec![0u8; max_len];
                match crate::net::sockets::recv(handle, &mut buf).await {
                    Ok(n) => {
                        let _ = self.write_to_memory(buf_ptr, &buf[..n]);
                        let _ = self.write_u32(nrecv_ptr, n as u32);
                        kprintln!("ruos: wasm fiber: sock recv n={}", n);
                        0
                    }
                    Err(e) => {
                        kprintln!("ruos: wasm fiber: sock recv err: {}", e);
                        8
                    }
                }
            }
            SuspendReason::SockSend { handle, bytes, nsent_ptr } => {
                kprintln!("ruos: wasm fiber: sock send len={}", bytes.len());
                match crate::net::sockets::send(handle, &bytes).await {
                    Ok(n) => {
                        let _ = self.write_u32(nsent_ptr, n as u32);
                        kprintln!("ruos: wasm fiber: sock sent n={}", n);
                        0
                    }
                    Err(e) => {
                        kprintln!("ruos: wasm fiber: sock send err: {}", e);
                        8
                    }
                }
            }
            // Other variants are implemented in Task 3.
            other => {
                kprintln!("ruos: wasm: SuspendReason {:?} not implemented", other);
                28 // EINVAL
            }
        }
    }

    fn find_fd_for_handle(&self, target: smoltcp::iface::SocketHandle) -> Option<u32> {
        use crate::wasm::state::FdEntry;
        let state = self.store.data();
        for (fd, slot) in state.fds.iter().enumerate() {
            if let Some(FdEntry::Socket(idx)) = slot {
                if crate::net::sockets::POOL.handle(*idx) == Some(target) {
                    return Some(fd as u32);
                }
            }
        }
        None
    }

    fn write_to_memory(&mut self, ptr: u32, bytes: &[u8]) -> Result<(), wasmi::Error> {
        let mem = self.instance
            .get_export(&self.store, "memory")
            .and_then(|e| e.into_memory())
            .ok_or_else(|| wasmi::Error::new("no memory export"))?;
        mem.write(&mut self.store, ptr as usize, bytes)
            .map_err(|_| wasmi::Error::new("memory write failed"))?;
        Ok(())
    }

    fn write_u32(&mut self, ptr: u32, val: u32) -> Result<(), wasmi::Error> {
        self.write_to_memory(ptr, &val.to_le_bytes())
    }

    fn error_to_exit(e: &wasmi::Error) -> i32 {
        if let Some(code) = e.kind().as_i32_exit_status() {
            return code;
        }
        kprintln!("ruos: wasm trap: {}", e);
        -1
    }
}
