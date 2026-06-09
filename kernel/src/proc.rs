//! Process registry: tracks live wasm fibers so userspace `ps`/`kill` can
//! see them. Cooperative kill — `kill(pid)` flips a flag; the target dies
//! at its next host-fn suspend (Fiber::run checks the flag after each
//! dispatch). This is the only termination signal the cooperative async
//! model can deliver without unwinding the wasm stack.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

#[derive(Clone)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
    pub start_tick: u64,
    pub kill: bool,
    /// Cumulative TSC cycles spent executing this fiber's wasm bursts.
    pub cpu_tsc: u64,
    /// Last-observed wasm linear-memory size in bytes.
    pub mem_bytes: u64,
    /// True for kernel async daemons (executor tasks, no wasm fiber). They show
    /// in `ps` with a `[bracketed]` name (Linux kernel-thread convention) and
    /// cannot be killed (`request_kill` refuses them).
    pub kernel: bool,
}

static REGISTRY: Mutex<BTreeMap<u32, ProcInfo>> = Mutex::new(BTreeMap::new());
static NEXT_PID: AtomicU32 = AtomicU32::new(1);

pub fn register(name: String) -> u32 {
    register_inner(name, false)
}

/// Register a long-lived kernel daemon (an `#[embassy_executor::task]` with no
/// wasm fiber: watchdog, sshd, dispatchers, workers). The name is shown
/// `[bracketed]` in `ps`; the daemon never exits, so it is never unregistered.
pub fn register_kernel(name: &str) -> u32 {
    register_inner(alloc::format!("[{}]", name), true)
}

fn register_inner(name: String, kernel: bool) -> u32 {
    let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
    let info = ProcInfo {
        pid,
        name,
        start_tick: crate::timer::ticks(),
        kill: false,
        cpu_tsc: 0,
        mem_bytes: 0,
        kernel,
    };
    REGISTRY.lock().insert(pid, info);
    pid
}

pub fn unregister(pid: u32) {
    REGISTRY.lock().remove(&pid);
}

pub fn list() -> Vec<ProcInfo> {
    REGISTRY.lock().values().cloned().collect()
}

/// Returns true if a process with `pid` exists, is killable (not a kernel
/// daemon), and was marked for kill. Returns false if the pid is unknown OR is a
/// kernel daemon (those are protected — you can't `kill` the watchdog/sshd).
pub fn request_kill(pid: u32) -> bool {
    let mut r = REGISTRY.lock();
    if let Some(p) = r.get_mut(&pid) {
        if p.kernel { return false; } // protected kernel daemon
        p.kill = true;
        true
    } else {
        false
    }
}

pub fn is_kill_pending(pid: u32) -> bool {
    REGISTRY.lock().get(&pid).map(|p| p.kill).unwrap_or(false)
}

/// Charge `delta` TSC cycles to `pid`'s cumulative CPU time. No-op if the pid
/// is gone (best-effort telemetry, never load-bearing).
pub fn add_cpu_tsc(pid: u32, delta: u64) {
    if let Some(p) = REGISTRY.lock().get_mut(&pid) {
        p.cpu_tsc = p.cpu_tsc.saturating_add(delta);
    }
}

/// Record the latest wasm linear-memory size for `pid`. No-op if unknown.
pub fn set_mem_bytes(pid: u32, bytes: u64) {
    if let Some(p) = REGISTRY.lock().get_mut(&pid) {
        p.mem_bytes = bytes;
    }
}
