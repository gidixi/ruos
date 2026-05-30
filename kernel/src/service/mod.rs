//! Service manager (init/systemd-lite).
//!
//! Registry of "named runnable units" — each entry maps a stable name
//! (e.g. `"ssh"`) to either a `.wasm` path or a builtin marker. The
//! userspace `service` tool reads the registry via three host fns
//! (`ruos_service_list`/`_start`/`_status`).
//!
//! Design notes:
//!
//! * The registry is a single `Vec<Service>` behind a `spin::Mutex`. The
//!   kernel is single-CPU + cooperative async, so the only contention is
//!   nested locks within the same fiber. Keep the critical sections tiny
//!   (no `.await` while the mutex is held).
//!
//! * `start(name)` posts to [`SERVICE_QUEUE`] and returns immediately —
//!   the actual fiber spawn happens in `executor::service_dispatcher_task`,
//!   mirroring the SSH PTY dispatcher pattern. Status moves to `Running`
//!   when the worker pulls the request; back to `Exited`/`Failed` when
//!   the fiber finishes.
//!
//! * No `stop()` of a running service: we have no generic cancellation
//!   primitive for wasm fibers (the cooperative-kill flag in
//!   `crate::proc` would need every host fn to check it, which is best-
//!   effort). The CLI subcommand is reserved but the kernel surface
//!   returns `ServiceError::NotSupported`. See note on `stop`.
//!
//! * SSH is treated as a "builtin" service: it is hardcoded-spawned from
//!   `boot::phases::userland::init`, so its registry entry has path
//!   `"<builtin>"` and gets marked `Running` directly after
//!   `ssh::spawn()` succeeds (see [`mark_running`]).

use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use core::task::Waker;
use spin::Mutex;

/// Marker path used for the SSH entry — it is not actually spawned via
/// the wasm dispatcher; the kernel boot phase starts the sunset task
/// directly. The userspace tool just displays this verbatim.
pub const BUILTIN_PATH: &str = "<builtin>";

#[derive(Clone, Debug)]
pub enum ServiceStatus {
    Idle,
    Running,
    Exited(i32),
    Failed(&'static str),
}

impl ServiceStatus {
    pub fn label(&self) -> &'static str {
        match self {
            ServiceStatus::Idle       => "Idle",
            ServiceStatus::Running    => "Running",
            ServiceStatus::Exited(_)  => "Exited",
            ServiceStatus::Failed(_)  => "Failed",
        }
    }
}

/// Internal registry record. Names and paths are `'static` so the boot
/// registration sites can use string literals; this keeps allocations
/// out of the registry's hot path (snapshots clone into `String`).
#[derive(Clone)]
pub struct Service {
    pub name:    &'static str,
    pub path:    &'static str,
    pub on_boot: bool,
    pub status:  ServiceStatus,
    pub pid:     Option<u32>,
    pub runs:    u32,
}

/// Owned snapshot shipped to userspace via `ruos_service_list`/`_status`.
#[derive(Clone)]
pub struct ServiceInfo {
    pub name:   String,
    pub path:   String,
    pub status: String,
    pub pid:    Option<u32>,
    pub runs:   u32,
}

#[derive(Debug)]
pub enum ServiceError {
    NotFound,
    AlreadyRunning,
    /// `stop()` of a running fiber — no cancellation primitive yet.
    NotSupported,
    Internal,
}

impl ServiceError {
    /// Errno-like wire code for the host fn ABI.
    pub fn errno(&self) -> i32 {
        match self {
            ServiceError::NotFound       => 1,
            ServiceError::AlreadyRunning => 2,
            ServiceError::NotSupported   => 3,
            ServiceError::Internal       => 99,
        }
    }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceError::NotFound       => write!(f, "service: not found"),
            ServiceError::AlreadyRunning => write!(f, "service: already running"),
            ServiceError::NotSupported   => write!(f, "service: operation not supported"),
            ServiceError::Internal       => write!(f, "service: internal error"),
        }
    }
}

static REGISTRY: Mutex<Vec<Service>> = Mutex::new(Vec::new());

/// Pending start requests + waker for the dispatcher task. Single-CPU,
/// so a simple `VecDeque` behind a `Mutex` is sufficient.
pub struct ServiceQueue {
    pub pending:       Mutex<VecDeque<&'static str>>,
    pub worker_waker:  Mutex<Option<Waker>>,
}

pub static SERVICE_QUEUE: ServiceQueue = ServiceQueue {
    pending:       Mutex::new(VecDeque::new()),
    worker_waker:  Mutex::new(None),
};

/// Add a service to the registry. Idempotent on `name` collisions — the
/// later registration is dropped with a warning so a typo in boot code
/// doesn't silently shadow an earlier entry.
pub fn register(name: &'static str, path: &'static str, on_boot: bool) {
    let mut r = REGISTRY.lock();
    if r.iter().any(|s| s.name == name) {
        crate::bwarn!("svc", "register: name '{}' already exists, skipping", name);
        return;
    }
    r.push(Service {
        name,
        path,
        on_boot,
        status: ServiceStatus::Idle,
        pid:    None,
        runs:   0,
    });
    crate::binfo!("svc", "register name={} path={} on_boot={}", name, path, on_boot);
}

/// Snapshot the whole registry for userspace display.
pub fn list() -> Vec<ServiceInfo> {
    REGISTRY.lock().iter().map(snapshot).collect()
}

/// Snapshot a single entry by name.
pub fn status(name: &str) -> Option<ServiceInfo> {
    REGISTRY.lock().iter().find(|s| s.name == name).map(snapshot)
}

fn snapshot(s: &Service) -> ServiceInfo {
    let status = match &s.status {
        ServiceStatus::Idle       => "Idle".to_string(),
        ServiceStatus::Running    => "Running".to_string(),
        ServiceStatus::Exited(c)  => alloc::format!("Exited({})", c),
        ServiceStatus::Failed(m)  => alloc::format!("Failed({})", m),
    };
    ServiceInfo {
        name:   s.name.to_string(),
        path:   s.path.to_string(),
        status,
        pid:    s.pid,
        runs:   s.runs,
    }
}

/// Queue a service start. Returns immediately; the dispatcher picks it up.
///
/// Refuses to enqueue a service whose status is already `Running` (the
/// caller would otherwise re-spawn the same wasm in parallel). Refuses
/// to start the SSH builtin via this path — it has no wasm payload.
pub fn start(name: &str) -> Result<(), ServiceError> {
    // Lookup + status update + path capture in one critical section.
    let static_name: &'static str = {
        let mut r = REGISTRY.lock();
        let entry = r.iter_mut().find(|s| s.name == name)
            .ok_or(ServiceError::NotFound)?;
        if matches!(entry.status, ServiceStatus::Running) {
            return Err(ServiceError::AlreadyRunning);
        }
        if entry.path == BUILTIN_PATH {
            // Builtins (e.g. SSH) cannot be (re)started through this
            // path — they have no wasm module to load. Treat as NotSupported
            // so the CLI can print a meaningful message.
            return Err(ServiceError::NotSupported);
        }
        entry.name
    };

    SERVICE_QUEUE.pending.lock().push_back(static_name);
    if let Some(w) = SERVICE_QUEUE.worker_waker.lock().take() {
        w.wake();
    }
    crate::binfo!("svc", "queued start name={}", static_name);
    Ok(())
}

/// Force `stop()` of a running fiber. NOT IMPLEMENTED — see module docs.
/// Reserved for symmetry; returns `NotSupported` so the CLI can print a
/// meaningful diagnostic without us having to grow new state.
pub fn stop(_name: &str) -> Result<(), ServiceError> {
    Err(ServiceError::NotSupported)
}

/// Resolve `name` to its static path. Used by the dispatcher task.
pub fn path_of(name: &str) -> Option<&'static str> {
    REGISTRY.lock().iter().find(|s| s.name == name).map(|s| s.path)
}

/// Transition `name` to `Running`, record its PID, bump the run counter.
/// Called by the dispatcher just before driving the fiber, and by
/// `boot::phases::userland` for the SSH builtin.
pub fn mark_running(name: &str, pid: u32) {
    let mut r = REGISTRY.lock();
    if let Some(s) = r.iter_mut().find(|s| s.name == name) {
        s.status = ServiceStatus::Running;
        s.pid    = Some(pid);
        s.runs   = s.runs.saturating_add(1);
    } else {
        crate::bwarn!("svc", "mark_running: unknown name '{}'", name);
    }
}

/// Transition `name` to `Exited(code)`, clear the PID.
pub fn mark_exited(name: &str, code: i32) {
    let mut r = REGISTRY.lock();
    if let Some(s) = r.iter_mut().find(|s| s.name == name) {
        s.status = ServiceStatus::Exited(code);
        s.pid    = None;
    }
}

/// Transition `name` to `Failed(reason)`, clear the PID.
pub fn mark_failed(name: &str, reason: &'static str) {
    let mut r = REGISTRY.lock();
    if let Some(s) = r.iter_mut().find(|s| s.name == name) {
        s.status = ServiceStatus::Failed(reason);
        s.pid    = None;
    }
}

/// Wait for a service-start request. Used by the executor's
/// `service_dispatcher_task`. Mirrors `WaitForRequest` in `exec_queue.rs`.
pub struct WaitForServiceRequest;

impl core::future::Future for WaitForServiceRequest {
    type Output = &'static str;

    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<&'static str> {
        if let Some(name) = SERVICE_QUEUE.pending.lock().pop_front() {
            return core::task::Poll::Ready(name);
        }
        *SERVICE_QUEUE.worker_waker.lock() = Some(cx.waker().clone());
        core::task::Poll::Pending
    }
}

/// Boot-time registry seeding. Called from `boot::phases::userland::init`
/// before the SSH server is spawned. Add new always-known services here.
pub fn init() {
    register("ssh", BUILTIN_PATH, true);
    // Example startable service for the userspace CLI — a no-op tool
    // already on the ISO. Lets `service start whoami` exercise the full
    // dispatcher path without shipping a dedicated fixture.
    register("whoami", "/bin/whoami.wasm", false);
}
