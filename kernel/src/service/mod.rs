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
pub mod topo;
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

/// Aggiorna enabled nel registry e posta la persistenza al dispatcher (le
/// scritture VFS sono async; le host fn wasmi sono sync → queue).
pub fn set_enabled(name: &str, on: bool) -> Result<(), ServiceError> {
    let has_file = {
        let mut r = UNITS.lock();
        let entry = r.iter_mut().find(|s| s.name == name).ok_or(ServiceError::NotFound)?;
        entry.enabled = on;
        entry.file.is_some()
    };
    if has_file {
        SERVICE_QUEUE.pending.lock().push_back(UnitReq::Persist(name.to_string()));
        if let Some(w) = SERVICE_QUEUE.worker_waker.lock().take() { w.wake(); }
    }
    Ok(())
}

/// Riscrive il file sorgente dell'unit (chiamata dal dispatcher, async).
pub async fn persist(name: &str) {
    let (file, text) = {
        let r = UNITS.lock();
        let Some(u) = r.iter().find(|s| s.name == name) else { return; };
        let Some(f) = u.file.clone() else { return; };
        let text = if f.ends_with(".json") { unitfile::to_json(u) } else { unitfile::to_yaml(u) };
        (f, text)
    };
    let flags = crate::vfs::OpenFlags::WRITE | crate::vfs::OpenFlags::CREATE | crate::vfs::OpenFlags::TRUNCATE;
    match crate::vfs::open(&file, flags).await {
        Ok(fd) => {
            let bytes = text.as_bytes();
            let mut off = 0;
            while off < bytes.len() {
                match crate::vfs::write(fd, &bytes[off..]).await {
                    Ok(n) if n > 0 => off += n,
                    _ => { crate::bwarn!("svc", "persist {}: short write", file); break; }
                }
            }
            let _ = crate::vfs::close(fd).await;
            crate::binfo!("svc", "persisted {}", file);
        }
        Err(_) => crate::bwarn!("svc", "persist {}: open failed", file),
    }
}

/// Ri-parsa la dir: nuove unit aggiunte, le file-sourced non-Running
/// vengono droppate e ricaricate (config fresca); le Running restano con
/// la config vecchia fino al prossimo restart (warn). Timer ri-armati.
pub async fn reload() {
    {
        let mut r = UNITS.lock();
        r.retain(|u| u.file.is_none()
            || matches!(u.status, UnitStatus::Running | UnitStatus::Restarting));
        for u in r.iter() {
            if u.file.is_some() {
                crate::bwarn!("svc", "reload: '{}' running — config refresh at next restart", u.name);
            }
        }
    }
    TIMERS.lock().retain(|t| t.file.is_none());
    // i duplicati con le Running superstiti vengono skippati con warn
    load_from_disk().await;
    let n = UNITS.lock().iter().filter(|u| u.file.is_some()).count();
    crate::binfo!("svc", "reload done ({} file units)", n);
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

/// Riga TSV estesa per `unit_list`/`unit_status`:
/// `name\tkind\tstatus\tpid\truns\trestarts\ttarget\tenabled\tpath\tfile\n`
pub fn list_tsv() -> String {
    let r = UNITS.lock();
    let mut out = String::new();
    for u in r.iter() { unit_row(&mut out, u); }
    out
}

pub fn status_tsv(name: &str) -> Option<String> {
    let r = UNITS.lock();
    r.iter().find(|u| u.name == name).map(|u| {
        let mut s = String::new(); unit_row(&mut s, u); s
    })
}

fn unit_row(out: &mut String, u: &Unit) {
    use core::fmt::Write;
    let status = match &u.status {
        UnitStatus::Exited(c) => alloc::format!("Exited({})", c),
        UnitStatus::Failed(m) => alloc::format!("Failed({})", m),
        other => other.label().to_string(),
    };
    let _ = writeln!(out, "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        u.name,
        match u.kind { UnitKind::Oneshot => "oneshot", UnitKind::Daemon => "daemon" },
        status,
        u.pid.map(|p| alloc::format!("{}", p)).unwrap_or_else(|| "-".to_string()),
        u.runs, u.restarts,
        match u.target { ActivateTarget::Boot => "boot", ActivateTarget::PostBoot => "post-boot", ActivateTarget::Manual => "manual" },
        u.enabled,
        u.path,
        u.file.as_deref().unwrap_or("-"));
}

/// `name\tunit\tschedule\tenabled\tnext_fire\tlast_fire\n` — fire in tick
/// (monotoni) o unix epoch (calendario), raw; il tool mostra schedule+raw.
pub fn timers_tsv() -> String {
    use core::fmt::Write;
    let g = TIMERS.lock();
    let mut out = String::new();
    for t in g.iter() {
        let sched = match &t.schedule {
            schedule::Schedule::EveryTicks(n) => alloc::format!("every {}s", n / 100),
            schedule::Schedule::BootPlus(n)   => alloc::format!("boot+{}s", n / 100),
            schedule::Schedule::Hourly { minute } => alloc::format!("hourly :{:02}", minute),
            schedule::Schedule::Daily { hour, minute } => alloc::format!("daily {:02}:{:02}", hour, minute),
            schedule::Schedule::Weekly { dow, hour, minute } => {
                const D: [&str; 7] = ["Sun","Mon","Tue","Wed","Thu","Fri","Sat"];
                alloc::format!("weekly {} {:02}:{:02}", D[*dow as usize % 7], hour, minute)
            }
        };
        let _ = writeln!(out, "{}\t{}\t{}\t{}\t{}\t{}",
            t.name, t.unit, sched, t.enabled, t.next_fire,
            t.last_fire.map(|v| alloc::format!("{}", v)).unwrap_or_else(|| "-".to_string()));
    }
    out
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

/// Parse di /mnt/etc/units/*.{yaml,yml,json} → registry. File malformato:
/// log + skip (le altre unit proseguono — no boot-loop). Dir assente: solo
/// builtin. Timer: next_fire armato qui.
pub async fn load_from_disk() {
    let entries = match crate::vfs::readdir(UNITS_DIR).await {
        Ok(e) => e,
        Err(_) => { crate::binfo!("svc", "{} absent — builtin only", UNITS_DIR); return; }
    };
    let ticks = crate::timer::ticks();
    let epoch = crate::rtc::to_unix_epoch(&crate::rtc::now());
    let mut n_units = 0u32;
    let mut n_timers = 0u32;
    for ent in entries {
        let fname = ent.name.as_str();
        let is_yaml = fname.ends_with(".yaml") || fname.ends_with(".yml");
        let is_json = fname.ends_with(".json");
        if !is_yaml && !is_json { continue; }
        let path = alloc::format!("{}/{}", UNITS_DIR, fname);
        let bytes = match crate::wasm::read_all(&path).await {
            Ok(b) => b,
            Err(_) => { crate::bwarn!("svc", "load: read {} failed", path); continue; }
        };
        let Ok(text) = core::str::from_utf8(&bytes) else {
            crate::bwarn!("svc", "load: {} not utf8", path); continue;
        };
        let doc = if is_yaml { yaml::parse(text) } else { json::parse(text) };
        let parsed = doc.and_then(|d| unitfile::build(&d, Some(&path)));
        match parsed {
            Ok(unitfile::Parsed::U(u)) => {
                let mut r = UNITS.lock();
                if r.iter().any(|x| x.name == u.name) {
                    crate::bwarn!("svc", "load: duplicate unit '{}' ({}) skipped", u.name, path);
                } else {
                    r.push(u); n_units += 1;
                }
            }
            Ok(unitfile::Parsed::T(mut t)) => {
                t.next_fire = schedule::compute_next(&t.schedule, epoch, ticks);
                let mut g = TIMERS.lock();
                if g.iter().any(|x| x.name == t.name) {
                    crate::bwarn!("svc", "load: duplicate timer '{}' ({}) skipped", t.name, path);
                } else {
                    g.push(t); n_timers += 1;
                }
            }
            Err(e) => crate::bwarn!("svc", "load: {} parse error: {}", path, e),
        }
    }
    crate::binfo!("svc", "loaded {} units + {} timers from {}", n_units, n_timers, UNITS_DIR);
}

/// "Su" = daemon/builtin → Running; oneshot → Exited(0).
pub fn is_up(name: &str) -> bool {
    let r = UNITS.lock();
    match r.iter().find(|s| s.name == name) {
        Some(u) => match u.kind {
            UnitKind::Daemon  => matches!(u.status, UnitStatus::Running),
            UnitKind::Oneshot => matches!(u.status, UnitStatus::Exited(0)),
        },
        None => false,
    }
}

fn is_failed(name: &str) -> bool {
    UNITS.lock().iter().find(|s| s.name == name)
        .map(|u| matches!(u.status, UnitStatus::Failed(_)))
        .unwrap_or(false)
}

/// Attiva le unit enabled del target `t` + chiusura transitiva dei requires,
/// in ordine topologico. Async: attende che le dep siano "su" (timeout 10s)
/// prima di avviare le dipendenti; requires fallito → dipendente
/// `Failed(dep)`. Vedi spec §6.
pub async fn activate_target(t: ActivateTarget) {
    // 1. set = enabled con target t + chiusura requires (snapshot, lock corto)
    let nodes: Vec<(String, Vec<String>)> = {
        let r = UNITS.lock();
        let mut set: Vec<String> = r.iter()
            .filter(|u| u.enabled && u.target == t)
            .map(|u| u.name.clone()).collect();
        let mut i = 0;
        while i < set.len() {                       // chiusura transitiva requires
            let reqs: Vec<String> = r.iter().find(|u| u.name == set[i])
                .map(|u| u.requires.clone()).unwrap_or_default();
            for q in reqs {
                if !set.contains(&q) && r.iter().any(|u| u.name == q) { set.push(q); }
            }
            i += 1;
        }
        set.iter().map(|n| {
            let u = r.iter().find(|u| u.name == *n).unwrap();
            let mut deps = u.after.clone();
            for q in &u.requires { if !deps.contains(q) { deps.push(q.clone()); } }
            (n.clone(), deps)
        }).collect()
    };
    if nodes.is_empty() { return; }

    // 2. topo-sort; ciclo → Failed(cycle)
    let (order, cyclic) = topo::topo_sort(&nodes);
    for n in &cyclic {
        crate::bwarn!("svc", "activate: dependency cycle at '{}'", n);
        mark_failed(n, "cycle");
    }

    // 3. avvio in ordine; attesa "su" delle dep (cap 10s)
    for name in order {
        let deps = nodes.iter().find(|(n, _)| *n == name)
            .map(|(_, d)| d.clone()).unwrap_or_default();
        let mut dep_failed = false;
        for d in &deps {
            // dep fuori registry (typo) → warn e prosegui
            if !UNITS.lock().iter().any(|u| u.name == *d) {
                crate::bwarn!("svc", "activate {}: unknown dep '{}'", name, d);
                continue;
            }
            let deadline = crate::timer::ticks() + 1_000;     // 10s
            while !is_up(d) && !is_failed(d) && crate::timer::ticks() < deadline {
                crate::executor::delay::Delay::ticks(10).await; // 100ms
            }
            if is_failed(d) { dep_failed = true; break; }
            if !is_up(d) {
                crate::bwarn!("svc", "activate {}: dep '{}' not up after 10s — proceeding", name, d);
            }
        }
        if dep_failed {
            crate::bwarn!("svc", "activate {}: required dep failed", name);
            mark_failed(&name, "dep");
            continue;
        }
        if is_up(&name) { continue; }                          // builtin già Running
        if let Err(e) = start(&name) {
            crate::bwarn!("svc", "activate {}: {}", name, e);
        }
    }
}

/// Seeding boot del registry. Chiamato da `boot::phases::userland::init`
/// prima dello spawn SSH.
pub fn init() {
    register("ssh",    BUILTIN_PATH,       UnitKind::Daemon,  ActivateTarget::Boot,   true);
    // Oneshot di esempio già sull'ISO: `service start whoami` / `unitctl
    // start whoami` esercitano il path dispatcher senza fixture dedicate.
    register("whoami", "/bin/whoami.wasm", UnitKind::Oneshot, ActivateTarget::Manual, false);
}
