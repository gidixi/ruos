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

// ── C2c: per-request parallel .cwasm exec ────────────────────────────────────

/// Execute a non-compositor `.cwasm` app in parallel on a ComputeApp core.
///
/// This is the C2c parallel exec path. It runs completely OUTSIDE the single-slot
/// `EXEC_QUEUE`, so multiple concurrent `.cwasm` execs (e.g. from two SSH shells)
/// are independent and safe — each has its own `Arc<ExecReply>`, its own
/// `run_app_on_core` task on a different core, and its own `proc` registration.
///
/// 1. Read the `.cwasm` bytes (async, on the calling fiber).
/// 2. `proc::register` the child (returns pid).
/// 3. `pick_compute_core()`:
///    - `Some(core)` → create `Arc<ExecReply>`, `spawn_on(core, run_app_on_core(...))`,
///      `reply.wait().await` (yields to BSP executor while AP runs), `proc::unregister`.
///    - `None` (1-2 core system) → run inline with `run_cwasm` (BSP-side fallback).
/// 4. Return the exit code to the calling fiber.
pub async fn exec_cwasm_parallel(
    path:    alloc::string::String,
    argv:    alloc::vec::Vec<alloc::vec::Vec<u8>>,
    term_pts: usize,
) -> i32 {
    let bytes = match crate::wasm::read_all(&path).await {
        Ok(b) => b,
        Err(_) => {
            kprintln!("ruos: exec_cwasm_parallel: read {} failed", path);
            return 127;
        }
    };

    let pid = crate::proc::register(
        alloc::string::String::from(path.trim_start_matches('/')),
    );
    // Foreground tracking (the wasmi exec_worker already does this): lets
    // cooked-VINTR and `kill <pid>` reach the child — component TUI apps
    // poll `is_kill_pending(foreground)` inside `host.poll-key`.
    crate::pty::set_foreground(term_pts, Some(pid));

    let code = match crate::executor::pick_compute_core() {
        Some(core) => {
            let reply = crate::executor::ExecReply::new();
            let boxed = bytes.into_boxed_slice();
            match crate::executor::spawn_on(
                core,
                crate::executor::run_app_on_core(boxed, argv, term_pts, reply.clone()),
            ) {
                Ok(()) => {
                    // Yield to the BSP executor while the AP runs the app.
                    // The BSP stays live for net/usb/ssh — THE GOAL.
                    crate::executor::ExecReplyFuture(reply).await
                }
                Err(_) => {
                    // Pool exhausted (unlikely with pool_size=4, but be safe).
                    crate::bwarn!("exec-ap",
                        "spawn_on({}) failed — pool busy; falling back to inline", core);
                    // The boxed bytes were moved into spawn_on and are now lost
                    // (SpawnError returns the token but we don't recover bytes here).
                    // Return 127 (command not runnable) as a safe failure.
                    reply.complete(127);
                    127
                }
            }
        }
        None => {
            // 1-2 core system: no ComputeApp AP — run inline on the calling fiber.
            crate::wasm::wt::run_cwasm(&bytes, argv, Some(term_pts))
        }
    };

    crate::pty::set_foreground(term_pts, None);
    crate::proc::unregister(pid);
    code
}

/// Per-host-call fuel budget. A pure-compute loop with no host calls burns this
/// and is killed; an I/O-bound module refuels every host call and runs forever.
const FUEL_PER_SLICE: u64 = 2_000_000_000;

pub struct Fiber {
    pub store: Store<RuntimeState>,
    instance: Instance,
    /// Registered pid from `crate::proc` — None for the boot shell fiber
    /// when the registry hasn't been wired yet. When Some, the run loop
    /// honors cooperative kill requests via `proc::is_kill_pending`.
    pid: Option<u32>,
}

impl Fiber {
    pub fn new(bytes: &[u8]) -> Result<Self, wasmi::Error> {
        // Eager compilation: compile all WASM functions during Module::new so
        // that call_resumable never triggers lazy translation mid-execution.
        let mut config = wasmi::Config::default();
        config.compilation_mode(wasmi::CompilationMode::Eager);
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, bytes)?;
        let mut store: Store<RuntimeState> = Store::new(&engine, RuntimeState::new());
        let _ = store.set_fuel(FUEL_PER_SLICE);
        store.limiter(|state| state as &mut dyn wasmi::ResourceLimiter);
        let mut linker: Linker<RuntimeState> = Linker::new(&engine);
        host::install(&mut linker)?;
        let instance = linker.instantiate_and_start(&mut store, &module)?;
        Ok(Self { store, instance, pid: None })
    }

    pub fn set_args(&mut self, args: alloc::vec::Vec<alloc::vec::Vec<u8>>) {
        self.store.data_mut().args = args;
    }

    pub fn set_cwd(&mut self, cwd: alloc::string::String) {
        let data = self.store.data_mut();
        // Expose cwd to the guest libc via PWD (read by `ruos_rt::init()` →
        // set_current_dir) so the child's RELATIVE paths resolve against the
        // shell's working dir. The kernel resolves WASI fd paths against "/", so
        // cwd must live in the guest libc — this is how it's seeded across exec.
        let pwd = alloc::format!("PWD={}", cwd).into_bytes();
        data.env.retain(|e| !e.starts_with(b"PWD="));
        data.env.push(pwd);
        data.cwd = cwd;
    }

    pub fn set_pid(&mut self, pid: u32) {
        self.pid = Some(pid);
    }

    /// Re-bind FDs 0/1/2 to /dev/pts/<idx> instead of the default pts/0.
    /// Used by the SSH server when spawning a shell on a fresh PTY.
    pub fn rebind_stdio_pty(&mut self, idx: usize) {
        use crate::vfs::{self, OpenFlags};
        use crate::wasm::state::FdEntry;
        let path = alloc::format!("/dev/pts/{}", idx);
        let mut fds = core::mem::take(&mut self.store.data_mut().fds);
        let mut bound = [false; 3];
        for slot in 0..3 {
            if let Some(FdEntry::Vfs(old)) = fds.get(slot).and_then(|s| s.as_ref()) {
                let old_fd = *old;
                let _ = vfs::block_on(vfs::close(old_fd));
            }
            match vfs::block_on(vfs::open(&path, OpenFlags::READ | OpenFlags::WRITE)) {
                Ok(fd) => {
                    bound[slot] = true;
                    fds[slot] = Some(FdEntry::Vfs(fd));
                }
                Err(e) => {
                    crate::bwarn!("wasm", "rebind_stdio_pty: open {} slot {} failed: {}",
                                  path, slot, e);
                    fds[slot] = None;
                }
            }
        }
        self.store.data_mut().fds = fds;
        crate::binfo!("wasm", "rebind_stdio_pty idx={} bound stdin={} stdout={} stderr={}",
                      idx, bound[0], bound[1], bound[2]);
    }

    /// Replace this fiber's FD `slot` (0=stdin, 1=stdout, 2=stderr) with the
    /// kernel VFS fd `fd`, closing the default `/dev/pts/0` entry it replaces.
    /// Used by the pipeline coordinator to wire a stage to a pipe end.
    pub fn bind_fd(&mut self, slot: usize, fd: crate::vfs::Fd) {
        use crate::vfs;
        use crate::wasm::state::FdEntry;
        let fds = &mut self.store.data_mut().fds;
        if slot >= fds.len() { return; }
        if let Some(FdEntry::Vfs(old)) = fds[slot].as_ref() {
            let old = *old;
            let _ = vfs::block_on(vfs::close(old));
        }
        fds[slot] = Some(FdEntry::Vfs(fd));
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
        let burst_start = crate::boot::clock::read_tsc();
        let mut inv = match start.call_resumable(&mut self.store, &[], &mut outputs) {
            Ok(i) => i,
            Err(e) => { kprintln!("ruos: wasm: call_resumable error: {}", e); return Self::error_to_exit(&e); }
        };
        if let Some(pid) = self.pid {
            crate::proc::add_cpu_tsc(pid, crate::boot::clock::read_tsc().saturating_sub(burst_start));
            crate::proc::set_mem_bytes(pid, self.current_mem_bytes());
        }

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
                            crate::wtrace!("ruos: wasm fiber: suspend {:?}", reason);
                            let errno = self.dispatch(reason).await;
                            // Cooperative kill: if userspace flipped our
                            // kill flag while we were suspended, exit now
                            // with the conventional SIGKILL code instead
                            // of resuming the wasm function.
                            if let Some(pid) = self.pid {
                                if crate::proc::is_kill_pending(pid) { return 137; }
                            }
                            // Refuel before every resume: I/O-bound modules
                            // that yield on every host call run forever;
                            // only a no-host-call tight loop can exhaust the
                            // budget between two consecutive host calls.
                            let _ = self.store.set_fuel(FUEL_PER_SLICE);
                            let resume_args = [Val::I32(errno)];
                            let mut next_outputs: [Val; 0] = [];
                            let burst_start = crate::boot::clock::read_tsc();
                            let resumed = state.resume(&mut self.store, &resume_args, &mut next_outputs);
                            if let Some(pid) = self.pid {
                                crate::proc::add_cpu_tsc(pid, crate::boot::clock::read_tsc().saturating_sub(burst_start));
                                crate::proc::set_mem_bytes(pid, self.current_mem_bytes());
                            }
                            inv = match resumed {
                                Ok(i) => i,
                                Err(e) => return Self::error_to_exit(&e),
                            };
                        }
                    }
                }
                ResumableCall::OutOfFuel(_) => {
                    kprintln!("wasm: task killed (fuel exhausted)");
                    return 137;
                }
            }
        }
    }

    async fn dispatch(&mut self, reason: SuspendReason) -> i32 {
        match reason {
            SuspendReason::Sleep { ticks, events_ptr, nevents_ptr } => {
                crate::wtrace!("ruos: wasm fiber: sleeping {} ticks", ticks);
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
                crate::wtrace!("ruos: wasm fiber: sleep done, writing 1 event");
                0
            }
            SuspendReason::SockAccept { handle, new_fd_ptr } => {
                crate::wtrace!("ruos: wasm fiber: sock accept waiting");
                match crate::net::sockets::accept(handle).await {
                    Ok(()) => {
                        // smoltcp's listen socket transitions to Established;
                        // there's no separate new socket. Write current fd.
                        let cur_fd: u32 = self.find_fd_for_handle(handle).unwrap_or(0);
                        let _ = self.write_u32(new_fd_ptr, cur_fd);
                        crate::wtrace!("ruos: wasm fiber: sock accepted fd={}", cur_fd);
                        0
                    }
                    Err(e) => {
                        kprintln!("ruos: wasm fiber: sock accept err: {}", e);
                        8
                    }
                }
            }
            SuspendReason::SockConnect { handle, remote, local_port } => {
                crate::wtrace!("ruos: wasm fiber: sock connect to {:?}:{}", remote.addr, remote.port);
                match crate::net::sockets::connect(handle, remote, local_port).await {
                    Ok(()) => {
                        crate::wtrace!("ruos: wasm fiber: sock connected");
                        0
                    }
                    Err(e) => {
                        kprintln!("ruos: wasm fiber: sock connect err: {}", e);
                        8
                    }
                }
            }
            SuspendReason::SockRecv { handle, buf_ptr, max_len, nrecv_ptr } => {
                crate::wtrace!("ruos: wasm fiber: sock recv max={}", max_len);
                let mut buf = alloc::vec![0u8; max_len];
                match crate::net::sockets::recv(handle, &mut buf).await {
                    Ok(n) => {
                        let _ = self.write_to_memory(buf_ptr, &buf[..n]);
                        let _ = self.write_u32(nrecv_ptr, n as u32);
                        crate::wtrace!("ruos: wasm fiber: sock recv n={}", n);
                        0
                    }
                    Err(e) => {
                        kprintln!("ruos: wasm fiber: sock recv err: {}", e);
                        8
                    }
                }
            }
            SuspendReason::SockSend { handle, bytes, nsent_ptr } => {
                crate::wtrace!("ruos: wasm fiber: sock send len={}", bytes.len());
                match crate::net::sockets::send(handle, &bytes).await {
                    Ok(n) => {
                        let _ = self.write_u32(nsent_ptr, n as u32);
                        crate::wtrace!("ruos: wasm fiber: sock sent n={}", n);
                        0
                    }
                    Err(e) => {
                        kprintln!("ruos: wasm fiber: sock send err: {}", e);
                        8
                    }
                }
            }
            SuspendReason::VfsRead { fd, buf_ptr, max_len, nread_ptr } => {
                let mut buf = alloc::vec![0u8; max_len];
                match crate::vfs::read(fd, &mut buf).await {
                    Ok(n) => {
                        let _ = self.write_to_memory(buf_ptr, &buf[..n]);
                        let _ = self.write_u32(nread_ptr, n as u32);
                        0
                    }
                    Err(_) => 8,
                }
            }
            SuspendReason::ReadStdinTimeout { pty_idx, buf_ptr, timeout_ticks } => {
                // Direct timed read on the PTY slave. This does NOT go through
                // `vfs::read`/`with_fd_take` (which removes the fd entry for the
                // duration and would strand it if the future were dropped on a
                // timeout). The pty read always resolves: byte (>=0), timeout
                // (-1), or EOF (-2).
                let r = crate::pty::slave_read_one_timeout(pty_idx, timeout_ticks).await;
                if r >= 0 {
                    let _ = self.write_to_memory(buf_ptr, &[r as u8]);
                    1
                } else if r == -2 {
                    -1 // EOF (stdin closed)
                } else {
                    0  // timeout
                }
            }
            SuspendReason::VfsWrite { fd, bytes, nwritten_ptr } => {
                match crate::vfs::write(fd, &bytes).await {
                    Ok(n) => {
                        let _ = self.write_u32(nwritten_ptr, n as u32);
                        0
                    }
                    Err(_) => 8,
                }
            }
            SuspendReason::VfsSeek { fd, offset, whence, newoffset_ptr } => {
                match crate::vfs::seek(fd, offset, whence).await {
                    Ok(n) => {
                        let _ = self.write_to_memory(newoffset_ptr, &(n as u64).to_le_bytes());
                        0
                    }
                    Err(_) => 8,
                }
            }
            SuspendReason::VfsClose { fd } => {
                let _ = crate::vfs::close(fd).await;
                0
            }
            SuspendReason::PathOpen { path, flags, opened_fd_ptr } => {
                match crate::vfs::open(&path, flags).await {
                    Ok(fd) => {
                        let state = self.store.data_mut();
                        let mut wfd: Option<u32> = None;
                        use crate::wasm::state::FdEntry;
                        for (i, slot) in state.fds.iter_mut().enumerate().skip(3) {
                            if slot.is_none() {
                                *slot = Some(FdEntry::Vfs(fd));
                                wfd = Some(i as u32);
                                break;
                            }
                        }
                        let wfd = match wfd {
                            Some(i) => i,
                            None if state.fds.len() < crate::wasm::state::MAX_FDS => {
                                state.fds.push(Some(FdEntry::Vfs(fd)));
                                (state.fds.len() - 1) as u32
                            }
                            None => return 33, // EMFILE (33) — fd table full
                        };
                        let _ = self.write_u32(opened_fd_ptr, wfd);
                        0
                    }
                    Err(_) => 44, // ENOENT
                }
            }
            SuspendReason::OpenDir { path, opened_fd_ptr } => {
                match crate::vfs::stat(&path).await {
                    Ok(s) if matches!(s.kind, crate::vfs::VfsKind::Dir) => {
                        let state = self.store.data_mut();
                        use crate::wasm::state::FdEntry;
                        // Allocate at fd >= 4: 0/1/2 are stdio and fd 3 is the
                        // virtual WASI preopen root ("/"). Handing out fd 3
                        // here would alias the preopen and corrupt std's path
                        // resolution (read_dir then fails with ENOENT).
                        let mut wfd: Option<u32> = None;
                        for (i, slot) in state.fds.iter_mut().enumerate().skip(4) {
                            if slot.is_none() {
                                *slot = Some(FdEntry::Dir(path.clone()));
                                wfd = Some(i as u32);
                                break;
                            }
                        }
                        let wfd = match wfd {
                            Some(i) => i,
                            None if state.fds.len() < crate::wasm::state::MAX_FDS => {
                                state.fds.push(Some(FdEntry::Dir(path.clone())));
                                (state.fds.len() - 1) as u32
                            }
                            None => return 33, // EMFILE (33) — fd table full
                        };
                        let _ = self.write_u32(opened_fd_ptr, wfd);
                        0
                    }
                    Ok(_)  => 54, // ENOTDIR — exists but not a directory
                    Err(_) => 44, // ENOENT
                }
            }
            SuspendReason::FdReadDir { path, cookie, buf_ptr, buf_len, bufused_ptr } => {
                let vfs_entries = match crate::vfs::readdir(&path).await {
                    Ok(v) => v,
                    Err(_) => {
                        let _ = self.write_u32(bufused_ptr, 0);
                        return 44; // ENOENT
                    }
                };
                // Full entry list: synthetic "." (idx 0), ".." (idx 1), then
                // the VFS entries. (name, __wasi_filetype_t).
                let mut all: alloc::vec::Vec<(alloc::string::String, u8)> =
                    alloc::vec::Vec::with_capacity(vfs_entries.len() + 2);
                all.push((alloc::string::String::from("."), 3));  // DIRECTORY
                all.push((alloc::string::String::from(".."), 3)); // DIRECTORY
                for e in vfs_entries.iter() {
                    let dtype: u8 = match e.kind {
                        crate::vfs::VfsKind::Dir    => 3, // DIRECTORY
                        crate::vfs::VfsKind::Reg    => 4, // REGULAR_FILE
                        crate::vfs::VfsKind::Device => 2, // CHARACTER_DEVICE
                    };
                    all.push((e.name.clone(), dtype));
                }
                // Pack 24-byte __wasi_dirent_t headers + names, skipping the
                // first `cookie` entries. d_next = i + 1 so a resume call with
                // cookie=K continues after entry K-1. A partial final entry is
                // allowed: write as many bytes as fit, then stop.
                let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
                let start = cookie as usize;
                'fill: for i in start..all.len() {
                    let (name, dtype) = &all[i];
                    let name_bytes = name.as_bytes();
                    if name_bytes.len() > u32::MAX as usize { continue; }
                    let mut hdr = [0u8; 24];
                    hdr[0..8].copy_from_slice(&((i as u64) + 1).to_le_bytes()); // d_next
                    // d_ino (8..16) left 0 — std::fs::read_dir doesn't use it.
                    hdr[16..20].copy_from_slice(&(name_bytes.len() as u32).to_le_bytes()); // d_namlen
                    hdr[20] = *dtype; // d_type
                    // pad (21..24) left 0.
                    for chunk in [&hdr[..], name_bytes] {
                        if out.len() >= buf_len { break 'fill; }
                        let space = buf_len - out.len();
                        if chunk.len() <= space {
                            out.extend_from_slice(chunk);
                        } else {
                            out.extend_from_slice(&chunk[..space]);
                            break 'fill;
                        }
                    }
                }
                let n = out.len().min(buf_len);
                let _ = self.write_to_memory(buf_ptr, &out[..n]);
                let _ = self.write_u32(bufused_ptr, n as u32);
                0
            }
            SuspendReason::Exec { path, argv, cwd, term_pts, exit_code_ptr } => {
                // C2c split: non-compositor .cwasm → parallel exec (per-request
                // Arc<ExecReply> + ComputeApp core, bypasses EXEC_QUEUE entirely);
                // compositor.cwasm + .wasm (wasmi) → single-slot EXEC_QUEUE path
                // (exec_worker_task handles them unchanged).
                //
                // The cwd arg is unused for .cwasm (run_cwasm gets argv only); it
                // is still forwarded to EXEC_QUEUE for .wasm (the wasmi fiber uses it).
                let code = if path.ends_with(".cwasm")
                    && !path.ends_with("compositor.cwasm")
                {
                    // Non-compositor .cwasm: take the parallel per-request path.
                    // Each concurrent exec has its own Arc<ExecReply> + AP task.
                    exec_cwasm_parallel(path, argv, term_pts).await
                } else {
                    // .wasm (wasmi fiber) or compositor.cwasm: delegate to the
                    // exec_worker_task via the single-slot EXEC_QUEUE (unchanged).
                    crate::wasm::exec_queue::EXEC_QUEUE
                        .post_and_wait(path, argv, cwd, term_pts)
                        .await
                };
                let _ = self.write_u32(exit_code_ptr, code as u32);
                0
            }
            SuspendReason::ExecPipeline { stages, cwd, term_pts, exit_code_ptr } => {
                let code = crate::wasm::pipeline::post_and_wait(stages, cwd, term_pts).await;
                let _ = self.write_u32(exit_code_ptr, code as u32);
                0
            }
            SuspendReason::PathUnlink { path } => {
                match crate::vfs::unlink(&path).await {
                    Ok(()) => 0,
                    Err(crate::vfs::VfsError::NotFound)    => 44, // ENOENT
                    Err(crate::vfs::VfsError::IsDirectory) => 31, // EISDIR
                    Err(_) => 8,                                  // EBADF/EIO bucket
                }
            }
            SuspendReason::PathMkdir { path } => {
                match crate::vfs::mkdir(&path).await {
                    Ok(()) => 0,
                    Err(crate::vfs::VfsError::AlreadyExists) => 20, // EEXIST
                    Err(crate::vfs::VfsError::NotFound)      => 44, // ENOENT
                    Err(crate::vfs::VfsError::NotDirectory)  => 54, // ENOTDIR
                    Err(_) => 8,
                }
            }
            SuspendReason::PathRmdir { path } => {
                match crate::vfs::rmdir(&path).await {
                    Ok(()) => 0,
                    Err(crate::vfs::VfsError::NotFound)     => 44, // ENOENT
                    Err(crate::vfs::VfsError::NotDirectory) => 54, // ENOTDIR
                    Err(crate::vfs::VfsError::NotPermitted) => 55, // ENOTEMPTY
                    Err(_) => 8,
                }
            }
            SuspendReason::PathFilestat { path, buf_ptr } => {
                match crate::vfs::stat(&path).await {
                    Ok(st) => {
                        let mut stat = [0u8; 64];
                        stat[16] = match st.kind {
                            crate::vfs::VfsKind::Reg    => 4,
                            crate::vfs::VfsKind::Dir    => 3,
                            crate::vfs::VfsKind::Device => 2,
                        };
                        stat[32..40].copy_from_slice(&st.size.to_le_bytes());
                        let _ = self.write_to_memory(buf_ptr, &stat);
                        0
                    }
                    Err(_) => 44, // ENOENT
                }
            }
            SuspendReason::PathRename { src, dst } => {
                match crate::vfs::rename(&src, &dst).await {
                    Ok(()) => 0,
                    Err(crate::vfs::VfsError::NotFound)      => 44,
                    Err(crate::vfs::VfsError::AlreadyExists) => 20,
                    Err(crate::vfs::VfsError::NotDirectory)  => 54,
                    Err(crate::vfs::VfsError::Invalid)       => 28, // EINVAL
                    Err(_) => 8,
                }
            }
            SuspendReason::Ping { target, timeout_ticks, latency_ms_ptr } => {
                match crate::net::icmp::ping(target, timeout_ticks).await {
                    Ok(ms) => {
                        let _ = self.write_u32(latency_ms_ptr, ms as u32);
                        0
                    }
                    Err(_) => 110, // ETIMEDOUT-ish
                }
            }
            SuspendReason::NetResolve { name, addrs_ptr, max_addrs, count_ptr } => {
                match crate::net::dns::resolve(&name).await {
                    Ok(addrs) => {
                        let count = addrs.len().min(max_addrs as usize);
                        for (i, ip) in addrs.iter().take(count).enumerate() {
                            let _ = self.write_to_memory(addrs_ptr + (i as u32 * 4), &ip.0);
                        }
                        let _ = self.write_u32(count_ptr, count as u32);
                        0
                    }
                    Err(_) => 44, // ENOENT
                }
            }
            SuspendReason::ReadDir { path, buf_ptr, buf_len, nread_ptr } => {
                let entries = match crate::vfs::readdir(&path).await {
                    Ok(v) => v,
                    Err(_) => {
                        let _ = self.write_u32(nread_ptr, 0);
                        return 44;
                    }
                };
                let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
                for e in entries.iter() {
                    let name_bytes = e.name.as_bytes();
                    if name_bytes.len() > u16::MAX as usize { continue; }
                    let kind_byte: u8 = match e.kind {
                        crate::vfs::VfsKind::Reg => 0,
                        crate::vfs::VfsKind::Dir => 1,
                        crate::vfs::VfsKind::Device => 2,
                    };
                    let entry_path = {
                        let mut s = path.clone();
                        if !s.ends_with('/') { s.push('/'); }
                        s.push_str(&e.name);
                        s
                    };
                    let size: u64 = match crate::vfs::stat(&entry_path).await {
                        Ok(s) => s.size,
                        Err(_) => 0,
                    };
                    out.push(kind_byte);
                    out.push(0);
                    out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
                    out.extend_from_slice(&size.to_le_bytes());
                    out.extend_from_slice(name_bytes);
                }
                let n = out.len().min(buf_len);
                let _ = self.write_to_memory(buf_ptr, &out[..n]);
                let _ = self.write_u32(nread_ptr, n as u32);
                0
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

    /// Current wasm linear-memory size in bytes (0 if no `memory` export).
    fn current_mem_bytes(&self) -> u64 {
        self.instance
            .get_export(&self.store, "memory")
            .and_then(|e| e.into_memory())
            .map(|m| m.data(&self.store).len() as u64)
            .unwrap_or(0)
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
