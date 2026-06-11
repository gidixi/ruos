//! Unit manager (init/systemd-lite).
//!
//! Registry di "unit" nominate — oneshot o daemon, builtin o `.wasm`/`.cwasm`
//! da file — con restart policy, dipendenze (after/requires), target di
//! attivazione (boot/post-boot/manual) e timer (vedi `schedule`).
//! Spec: docs/superpowers/specs/2026-06-09-init-units-timers-design.md.
//!
//! Design notes:
//!
//! * Owner = BSP. Due registry `UNITS`/`TIMERS` dietro `spin::Mutex`,
//!   sezioni critiche minime, mai `.await` con il lock tenuto.
//!
//! * `start(name)` posta `UnitReq::Start` in [`SERVICE_QUEUE`] e ritorna
//!   subito — spawn/exec avvengono in `executor::service_dispatcher_task`
//!   (oneshot inline) o in un `unit_runner_task` (daemon supervisionati).
//!   `Persist`/`Reload` viaggiano sulla stessa queue: le host fn wasmi sono
//!   sync, le scritture VFS sono async → le delega il dispatcher.
//!
//! * `stop(name)` è cooperativo: `stop_requested` + `proc::request_kill` —
//!   il child esce al prossimo check del kill flag (host call); il runner
//!   vede il flag e non riavvia. Best-effort per design (no preemption).
//!
//! * SSH resta un "builtin": spawnato da `boot::phases::userland::init`,
//!   entry con path `"<builtin>"`, marcato `Running` dal boot phase.

use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use core::task::Waker;
use spin::Mutex;

pub mod schedule;
pub mod unitfile;
pub mod yaml;
pub mod json;
#[cfg(feature = "boot-checks")]
pub mod checks;

/// Marker path per le entry builtin (es. SSH) — non spawnabili dal
/// dispatcher, il kernel le avvia direttamente al boot.
pub const BUILTIN_PATH: &str = "<builtin>";

/// Max daemon supervisionati in parallelo (pool di `unit_runner_task`).
pub const MAX_DAEMONS: usize = 8;

/// Directory dei file unit persistenti.
pub const UNITS_DIR: &str = "/mnt/etc/units";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitKind { Oneshot, Daemon }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartPolicy { No, OnFailure, Always }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivateTarget { Boot, PostBoot, Manual }

#[derive(Clone, Debug)]
pub enum UnitStatus {
    Idle,
    Running,
    Exited(i32),
    Failed(&'static str),
    Restarting,
}

impl UnitStatus {
    pub fn label(&self) -> &'static str {
        match self {
            UnitStatus::Idle       => "Idle",
            UnitStatus::Running    => "Running",
            UnitStatus::Exited(_)  => "Exited",
            UnitStatus::Failed(_)  => "Failed",
            UnitStatus::Restarting => "Restarting",
        }
    }
}

#[derive(Clone)]
pub struct Unit {
    pub name:     String,
    pub path:     String,            // "/mnt/bin/foo.wasm|.cwasm" | "<builtin>"
    pub kind:     UnitKind,
    pub restart:  RestartPolicy,
    pub after:    Vec<String>,
    pub requires: Vec<String>,
    pub target:   ActivateTarget,
    pub enabled:  bool,
    pub status:   UnitStatus,
    pub pid:      Option<u32>,
    pub runs:     u32,
    pub restarts: u32,
    pub stop_requested: bool,
    /// File sorgente in /mnt/etc/units (None = registrata da codice/CLI).
    pub file:     Option<String>,
}

#[derive(Clone)]
pub struct Timer {
    pub name:      String,
    pub unit:      String,
    pub schedule:  schedule::Schedule,
    pub enabled:   bool,
    /// Tick (EveryTicks/BootPlus) o unix epoch (calendario) del prossimo scatto.
    pub next_fire: u64,
    pub last_fire: Option<u64>,
    pub file:      Option<String>,
}

/// Snapshot owned per la ABI `service_list`/`service_status` (tool legacy).
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
    NotSupported,
    NoSlot,
    Parse,
    Internal,
}

impl ServiceError {
    /// Errno-like wire code per la host fn ABI.
    pub fn errno(&self) -> i32 {
        match self {
            ServiceError::NotFound       => 1,
            ServiceError::AlreadyRunning => 2,
            ServiceError::NotSupported   => 3,
            ServiceError::NoSlot         => 4,
            ServiceError::Parse          => 5,
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
            ServiceError::NoSlot         => write!(f, "service: no free daemon slot"),
            ServiceError::Parse          => write!(f, "service: parse error"),
            ServiceError::Internal       => write!(f, "service: internal error"),
        }
    }
}

static UNITS:  Mutex<Vec<Unit>>  = Mutex::new(Vec::new());
static TIMERS: Mutex<Vec<Timer>> = Mutex::new(Vec::new());

/// Richiesta per il dispatcher (BSP). Le host fn cross-core postano qui e
/// svegliano il worker — non mutano i registry direttamente.
#[derive(Clone, Debug)]
pub enum UnitReq {
    Start(String),
    Persist(String),
    Reload,
}

pub struct ServiceQueue {
    pub pending:      Mutex<VecDeque<UnitReq>>,
    pub worker_waker: Mutex<Option<Waker>>,
}

pub static SERVICE_QUEUE: ServiceQueue = ServiceQueue {
    pending:      Mutex::new(VecDeque::new()),
    worker_waker: Mutex::new(None),
};

/// Registra una unit da codice (builtin/seed). Idempotente sul nome.
pub fn register(name: &str, path: &str, kind: UnitKind, target: ActivateTarget, enabled: bool) {
    let mut r = UNITS.lock();
    if r.iter().any(|s| s.name == name) {
        crate::bwarn!("svc", "register: name '{}' already exists, skipping", name);
        return;
    }
    r.push(Unit {
        name: name.to_string(), path: path.to_string(),
        kind, restart: RestartPolicy::No, after: Vec::new(), requires: Vec::new(),
        target, enabled,
        status: UnitStatus::Idle, pid: None, runs: 0, restarts: 0,
        stop_requested: false, file: None,
    });
    crate::binfo!("svc", "register name={} path={}", name, path);
}

/// Snapshot dell'intero registry per il tool `service` legacy.
pub fn list() -> Vec<ServiceInfo> {
    UNITS.lock().iter().map(snapshot).collect()
}

/// Snapshot di una singola entry per nome.
pub fn status(name: &str) -> Option<ServiceInfo> {
    UNITS.lock().iter().find(|s| s.name == name).map(snapshot)
}

fn snapshot(s: &Unit) -> ServiceInfo {
    let status = match &s.status {
        UnitStatus::Exited(c) => alloc::format!("Exited({})", c),
        UnitStatus::Failed(m) => alloc::format!("Failed({})", m),
        other => other.label().to_string(),
    };
    ServiceInfo {
        name:   s.name.clone(),
        path:   s.path.clone(),
        status,
        pid:    s.pid,
        runs:   s.runs,
    }
}

/// Accoda uno start. Ritorna subito; il dispatcher fa il resto.
pub fn start(name: &str) -> Result<(), ServiceError> {
    {
        let r = UNITS.lock();
        let entry = r.iter().find(|s| s.name == name)
            .ok_or(ServiceError::NotFound)?;
        if matches!(entry.status, UnitStatus::Running | UnitStatus::Restarting) {
            return Err(ServiceError::AlreadyRunning);
        }
        if entry.path == BUILTIN_PATH {
            return Err(ServiceError::NotSupported);
        }
    }
    SERVICE_QUEUE.pending.lock().push_back(UnitReq::Start(name.to_string()));
    if let Some(w) = SERVICE_QUEUE.worker_waker.lock().take() {
        w.wake();
    }
    crate::binfo!("svc", "queued start name={}", name);
    Ok(())
}

/// Stop cooperativo: setta stop_requested e (se c'è un pid) request_kill.
/// Best-effort: un daemon in CPU-loop puro senza host call non si ferma.
pub fn stop(name: &str) -> Result<(), ServiceError> {
    let pid = {
        let mut r = UNITS.lock();
        let entry = r.iter_mut().find(|s| s.name == name)
            .ok_or(ServiceError::NotFound)?;
        if entry.path == BUILTIN_PATH { return Err(ServiceError::NotSupported); }
        entry.stop_requested = true;
        entry.pid
    };
    if let Some(pid) = pid { let _ = crate::proc::request_kill(pid); }
    Ok(())
}

/// Risolve `name` nel path. Usato dal dispatcher e dal runner.
pub fn path_of(name: &str) -> Option<String> {
    UNITS.lock().iter().find(|s| s.name == name).map(|s| s.path.clone())
}

/// Snapshot (kind, restart, path) per dispatcher/runner — una sezione critica.
pub fn exec_info_of(name: &str) -> Option<(UnitKind, RestartPolicy, String)> {
    UNITS.lock().iter().find(|s| s.name == name)
        .map(|s| (s.kind, s.restart, s.path.clone()))
}

/// Transita a `Running`, registra pid, bumpa il run counter.
pub fn mark_running(name: &str, pid: u32) {
    let mut r = UNITS.lock();
    if let Some(s) = r.iter_mut().find(|s| s.name == name) {
        s.status = UnitStatus::Running;
        s.pid    = Some(pid);
        s.runs   = s.runs.saturating_add(1);
    } else {
        crate::bwarn!("svc", "mark_running: unknown name '{}'", name);
    }
}

/// Transita a `Exited(code)`, azzera pid e stop_requested.
pub fn mark_exited(name: &str, code: i32) {
    let mut r = UNITS.lock();
    if let Some(s) = r.iter_mut().find(|s| s.name == name) {
        s.status = UnitStatus::Exited(code);
        s.pid    = None;
        s.stop_requested = false;
    }
}

/// Transita a `Failed(reason)`, azzera pid.
pub fn mark_failed(name: &str, reason: &'static str) {
    let mut r = UNITS.lock();
    if let Some(s) = r.iter_mut().find(|s| s.name == name) {
        s.status = UnitStatus::Failed(reason);
        s.pid    = None;
    }
}

/// Transita a `Restarting` (tra exit e re-spawn del runner), azzera pid.
pub fn mark_restarting(name: &str) {
    let mut r = UNITS.lock();
    if let Some(s) = r.iter_mut().find(|s| s.name == name) {
        s.status = UnitStatus::Restarting;
        s.pid    = None;
    }
}

/// Setta uno status arbitrario. Ritorna false se la unit non esiste.
pub fn mark_status(name: &str, st: UnitStatus) -> bool {
    let mut r = UNITS.lock();
    match r.iter_mut().find(|s| s.name == name) {
        Some(s) => { s.status = st; true }
        None => false,
    }
}

/// Legge e consuma il flag stop. Chiamata dal runner all'uscita del child.
pub fn take_stop_requested(name: &str) -> bool {
    let mut r = UNITS.lock();
    r.iter_mut().find(|s| s.name == name)
        .map(|s| core::mem::replace(&mut s.stop_requested, false))
        .unwrap_or(false)
}

/// Incrementa e ritorna il contatore restart (per il backoff).
pub fn bump_restarts(name: &str) -> u32 {
    let mut r = UNITS.lock();
    r.iter_mut().find(|s| s.name == name)
        .map(|s| { s.restarts = s.restarts.saturating_add(1); s.restarts })
        .unwrap_or(0)
}

/// Azzera il contatore restart (daemon rimasto su oltre la soglia).
pub fn reset_restarts(name: &str) {
    let mut r = UNITS.lock();
    if let Some(s) = r.iter_mut().find(|s| s.name == name) { s.restarts = 0; }
}

/// Snapshot dei timer enabled per lo scheduler: (idx, schedule, next_fire).
pub fn timers_due_snapshot() -> Vec<(usize, schedule::Schedule, u64)> {
    TIMERS.lock().iter().enumerate()
        .filter(|(_, t)| t.enabled)
        .map(|(i, t)| (i, t.schedule.clone(), t.next_fire))
        .collect()
}

/// Registra lo scatto del timer `idx`: last_fire=now, next_fire ricalcolato
/// (sempre futuro); BootPlus si disabilita (one-shot). Ritorna l'unit da
/// avviare.
pub fn timer_fired(idx: usize, epoch_now: u64, ticks_now: u64) -> Option<String> {
    let mut g = TIMERS.lock();
    let t = g.get_mut(idx)?;
    t.last_fire = Some(match t.schedule {
        schedule::Schedule::EveryTicks(_) | schedule::Schedule::BootPlus(_) => ticks_now,
        _ => epoch_now,
    });
    t.next_fire = schedule::compute_next(&t.schedule, epoch_now, ticks_now);
    if matches!(t.schedule, schedule::Schedule::BootPlus(_)) { t.enabled = false; }
    Some(t.unit.clone())
}

/// Attesa di una richiesta dal dispatcher. Pattern `WaitForRequest`.
pub struct WaitForServiceRequest;

impl core::future::Future for WaitForServiceRequest {
    type Output = UnitReq;

    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<UnitReq> {
        if let Some(req) = SERVICE_QUEUE.pending.lock().pop_front() {
            return core::task::Poll::Ready(req);
        }
        *SERVICE_QUEUE.worker_waker.lock() = Some(cx.waker().clone());
        core::task::Poll::Pending
    }
}

/// Carica /mnt/etc/units/*.{yaml,yml,json}. Implementazione al Task 11
/// del piano init-units (stub per far girare init_units_task prima).
pub async fn load_from_disk() {}

/// Seeding boot del registry. Chiamato da `boot::phases::userland::init`
/// prima dello spawn SSH.
pub fn init() {
    register("ssh",    BUILTIN_PATH,       UnitKind::Daemon,  ActivateTarget::Boot,   true);
    // Oneshot di esempio già sull'ISO: `service start whoami` / `unitctl
    // start whoami` esercitano il path dispatcher senza fixture dedicate.
    register("whoami", "/bin/whoami.wasm", UnitKind::Oneshot, ActivateTarget::Manual, false);
}
