# Init System (units, timers, supervisione) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Estendere il service manager minimale (`kernel/src/service/mod.rs`) in un init system completo: unit oneshot/daemon con restart policy + backoff, dipendenze (after/requires) con topo-sort, timer (intervallo monotono + calendario RTC), config su file `/mnt/etc/units/*.{yaml,json}`, CLI `unitctl`.

**Architecture:** Owner = BSP (registry `UNITS`/`TIMERS` dietro `spin::Mutex`, mai `.await` sotto lock). Tre task async sul BSP: dispatcher esteso (start/persist/reload via `UNIT_QUEUE`), runner pool per daemon (restart/backoff), scheduler (polling 1s). Esecuzione riusa il routing esistente: `.cwasm` → `exec_cwasm_parallel` → compute core; `.wasm` → wasmi `Fiber` sul BSP. Spec: `docs/superpowers/specs/2026-06-09-init-units-timers-design.md`.

**Tech Stack:** Rust `no_std` kernel (`x86_64-unknown-none`), embassy-executor, wasmi host fns (`ruos.*`), tool `.wasm` `wasm32-wasip1`. Parser YAML-subset/JSON hand-written, zero dipendenze.

**Build/verifica (questo clone):** host build = WSL distro `Ubuntu-22.04`, repo a `/mnt/w/Work/GitHub/ruos`.
- Compile check (per task): `wsl -d Ubuntu-22.04 -u root -e bash -c 'export PATH=$HOME/.cargo/bin:$PATH; cd /mnt/w/Work/GitHub/ruos/kernel && cargo check 2>&1 | tail -5'` → atteso `Finished` (solo warning).
- Boot-checks (fine fase): `wsl ... 'cd /mnt/w/Work/GitHub/ruos && make iso CARGO_FEATURES=boot-checks && make run-test'` → atteso PASS (stringa `HELLO` del Makefile) e nessun panic dei check.
- Tool wasm: `wsl ... 'cd /mnt/w/Work/GitHub/ruos/user && cargo build --target wasm32-wasip1 --release -p unitctl'`.

**TDD in kernel `no_std`:** niente `cargo test` per il crate kernel. Il ciclo red/green è: (1) scrivi il boot-check che chiama la funzione nuova → `cargo check` FALLISCE (E0425 funzione inesistente) = red; (2) implementa → `cargo check` passa; (3) la verifica semantica dei check gira in QEMU a fine fase (`make run-test` con `boot-checks`). I check usano `assert!`/`assert_eq!` (panic = fail visibile nel run di test).

**Regole repo:** ogni commit ha la sua entry `CHANGELOG/NN-26-06-11-<slug>.md` (controllare il numero più alto esistente prima di crearla — al momento della stesura il prossimo libero è 429). Nuove host fn app-facing → aggiornare `docs/api/ruos.md` nello stesso commit. Branch di lavoro: `feat/init-units` (da creare al Task 1; NON committare su `main`).

---

## File structure

| File | Ruolo |
|------|-------|
| `kernel/src/service/mod.rs` | (esteso) modello Unit/Timer, registry UNITS/TIMERS, queue, lifecycle marks, activate_target |
| `kernel/src/service/yaml.rs` | (nuovo) parser YAML-subset → `UnitDoc` |
| `kernel/src/service/json.rs` | (nuovo) parser JSON-subset → `UnitDoc` |
| `kernel/src/service/unitfile.rs` | (nuovo) `UnitDoc` → `Unit`/`Timer` (builder + validazione + serializzatori per persistenza) |
| `kernel/src/service/schedule.rs` | (nuovo) `Schedule`, `schedule_parse`, `compute_next`, `backoff_ticks` |
| `kernel/src/service/topo.rs` | (nuovo) topo-sort Kahn |
| `kernel/src/service/checks.rs` | (nuovo) boot-checks (gated `boot-checks`) |
| `kernel/src/executor/mod.rs` | (modificato) dispatcher esteso, `daemon_runner_task` pool, `scheduler_task`, `init_units_task` |
| `kernel/src/wasm/fiber.rs` | (modificato) factor `exec_cwasm_inner` con pid iniettato |
| `kernel/src/wasm/host/unit.rs` | (nuovo) host fns `ruos.unit_*` / `timer_list` |
| `kernel/src/wasm/host/mod.rs` | (modificato) wiring `unit::link` |
| `kernel/src/boot/phases/userland.rs` | (modificato) hook boot-checks service |
| `user/unitctl/{Cargo.toml,src/main.rs}` | (nuovo) CLI |
| `user/Cargo.toml`, `Makefile` | (modificati) wiring build `unitctl` |
| `docs/api/ruos.md` | (modificato) doc host fns |

---

### Task 1: Branch + modello dati Unit/Timer + migrazione registry a `String`

**Files:**
- Modify: `kernel/src/service/mod.rs`
- Modify: `kernel/src/executor/mod.rs:739-778` (service_dispatcher_task)
- Modify: `kernel/src/wasm/host/service.rs` (nessun cambio funzionale: snapshot invariato)
- Modify: `kernel/src/boot/phases/userland.rs` (nessun cambio funzionale: `service::init()` resta)

- [ ] **Step 1: crea il branch**

```bash
git -C W:\Work\GitHub\ruos switch -c feat/init-units
```

- [ ] **Step 2: sostituisci il modello in `service/mod.rs`**

Sostituire `Service`/`ServiceStatus` e la queue con il modello spec. Mantenere i nomi pubblici già usati altrove (`list`, `status`, `start`, `mark_running`, `mark_exited`, `mark_failed`, `path_of`, `init`, `BUILTIN_PATH`, `ServiceInfo`, `ServiceError`, `WaitForServiceRequest`) così `host/service.rs` e `userland.rs` continuano a compilare. Modifiche:

```rust
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
            UnitStatus::Idle        => "Idle",
            UnitStatus::Running     => "Running",
            UnitStatus::Exited(_)   => "Exited",
            UnitStatus::Failed(_)   => "Failed",
            UnitStatus::Restarting  => "Restarting",
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
    pub schedule:  crate::service::schedule::Schedule,
    pub enabled:   bool,
    pub next_fire: u64,              // tick (EveryTicks/BootPlus) o epoch (calendario)
    pub last_fire: Option<u64>,
    pub file:      Option<String>,
}

static UNITS:  Mutex<Vec<Unit>>  = Mutex::new(Vec::new());
static TIMERS: Mutex<Vec<Timer>> = Mutex::new(Vec::new());
```

Il vecchio `static REGISTRY: Mutex<Vec<Service>>` e `ServiceStatus` spariscono: usare `UnitStatus` ovunque (il compilatore segnala i call-site da aggiornare). Per far compilare `Timer` già in questo task, creare `service/schedule.rs` con il solo enum (il resto arriva al Task 4):

```rust
//! Schedule dei timer (parse + next-fire). Vedi spec init-units §1/§4.
use alloc::string::String;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Schedule {
    EveryTicks(u64),
    BootPlus(u64),
    Hourly { minute: u8 },
    Daily  { hour: u8, minute: u8 },
    Weekly { dow: u8, hour: u8, minute: u8 },   // 0=Sun..6=Sat
}
```

e in `mod.rs`: `pub mod schedule;`.

La queue diventa owned + multi-request (servirà a persist/reload nei task successivi):

```rust
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
```

`WaitForServiceRequest::Output` diventa `UnitReq`. `start()` posta `UnitReq::Start(name.to_string())`.

`register` diventa il seeding builtin/codice:

```rust
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

pub fn init() {
    register("ssh",    BUILTIN_PATH,       UnitKind::Daemon,  ActivateTarget::Boot,   true);
    register("whoami", "/bin/whoami.wasm", UnitKind::Oneshot, ActivateTarget::Manual, false);
}
```

`list`/`status`/`snapshot`: la `ServiceInfo` resta identica (name/path/status/pid/runs) — `snapshot` mappa da `Unit`. `path_of` ritorna `Option<String>` (clone). `mark_running`/`mark_exited`/`mark_failed`: firma invariata, operano su `UNITS`; `mark_exited` azzera anche `stop_requested`. Aggiungere:

```rust
pub fn mark_restarting(name: &str) {
    let mut r = UNITS.lock();
    if let Some(s) = r.iter_mut().find(|s| s.name == name) {
        s.status = UnitStatus::Restarting;
        s.pid = None;
    }
}

/// Legge e consuma il flag stop. Chiamata dal runner all'uscita del child.
pub fn take_stop_requested(name: &str) -> bool {
    let mut r = UNITS.lock();
    r.iter_mut().find(|s| s.name == name)
        .map(|s| core::mem::replace(&mut s.stop_requested, false))
        .unwrap_or(false)
}

/// Snapshot (kind, restart, path) per il runner — una sola sezione critica.
pub fn exec_info_of(name: &str) -> Option<(UnitKind, RestartPolicy, String)> {
    UNITS.lock().iter().find(|s| s.name == name)
        .map(|s| (s.kind, s.restart, s.path.clone()))
}

/// Incrementa e ritorna il contatore restart (per il backoff).
pub fn bump_restarts(name: &str) -> u32 {
    let mut r = UNITS.lock();
    r.iter_mut().find(|s| s.name == name)
        .map(|s| { s.restarts = s.restarts.saturating_add(1); s.restarts })
        .unwrap_or(0)
}
```

`ServiceError`: aggiungere varianti `NoSlot` (errno 4) e `Parse` (errno 5) + Display (`"service: no free daemon slot"`, `"service: parse error"`).

`stop()` (sostituisce il `NotSupported` attuale):

```rust
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
```

- [ ] **Step 3: adatta `service_dispatcher_task`** (`kernel/src/executor/mod.rs:739`)

Il loop ora matcha `UnitReq` (Persist/Reload diventano `bwarn!("svc", "not yet implemented")` fino ai Task 10-11) e `Start(name)` esegue il path inline attuale con `String`:

```rust
loop {
    let req = WaitForServiceRequest.await;
    let name = match req {
        crate::service::UnitReq::Start(n) => n,
        other => { crate::bwarn!("svc", "dispatcher: {:?} not yet implemented", other); continue; }
    };
    let path = match path_of(&name) {
        Some(p) => p,
        None => { crate::bwarn!("svc", "dispatcher: unknown name '{}'", name); continue; }
    };
    let bytes = match crate::wasm::read_all(&path).await {
        Ok(b) => b,
        Err(_) => { kprintln!("svc: read {} failed", path); mark_failed(&name, "read"); continue; }
    };
    let mut fb = match crate::wasm::fiber::Fiber::new(&bytes) {
        Ok(f) => f,
        Err(e) => { kprintln!("svc: instantiate {}: {}", path, e); mark_failed(&name, "instantiate"); continue; }
    };
    fb.set_args(alloc::vec![name.as_bytes().to_vec()]);
    let pid = crate::proc::register(name.clone());
    fb.set_pid(pid);
    mark_running(&name, pid);
    crate::binfo!("svc", "start name={} pid={} path={}", name, pid, path);
    let code = fb.run().await;
    crate::proc::unregister(pid);
    mark_exited(&name, code);
    crate::binfo!("svc", "exit name={} code={}", name, code);
}
```

- [ ] **Step 4: cargo check**

Run (WSL, vedi header). Expected: `Finished` — sistemare ogni call-site rotto segnalato dal compilatore (in particolare `host/service.rs` se usa `ServiceStatus`: rinominare in `UnitStatus`).

- [ ] **Step 5: changelog + commit**

Entry `CHANGELOG/429-26-06-11-init-units-model.md` (verificare NN: usare max+1 reale):

```markdown
# 429 — init: modello Unit/Timer, registry a String, queue multi-request

**Data:** 2026-06-11

## Cosa
service/mod.rs: Service→Unit (kind/restart/after/requires/target/enabled/
restarts/stop_requested/file), UnitStatus(+Restarting), Timer+Schedule
(placeholder), UNITS/TIMERS, UnitReq{Start,Persist,Reload}, stop()
cooperativo via request_kill. Dispatcher adattato. Fase 1 della spec
init-units-timers.

## Perché
Base dati per init system (spec 2026-06-09-init-units-timers-design.md).

## File toccati
- kernel/src/service/mod.rs
- kernel/src/service/schedule.rs
- kernel/src/executor/mod.rs
```

```bash
git add kernel/src/service/ kernel/src/executor/mod.rs kernel/src/wasm/host/service.rs CHANGELOG/429-*.md
git commit -m "feat(init): modello Unit/Timer + registry String + stop cooperativo"
```

---

### Task 2: `UnitDoc` + parser YAML-subset (TDD via boot-checks)

**Files:**
- Create: `kernel/src/service/yaml.rs`
- Create: `kernel/src/service/checks.rs`
- Create: `kernel/src/service/unitfile.rs` (in questo task solo `Val`/`UnitDoc` — il builder arriva al Task 5; entrambi i parser importano da qui, niente cicli di modulo)
- Modify: `kernel/src/service/mod.rs` (+`pub mod unitfile; pub mod yaml;` + hook checks)
- Modify: `kernel/src/boot/phases/userland.rs` (chiamata checks gated)

- [ ] **Step 1: scrivi `UnitDoc` in `unitfile.rs`**

```rust
//! UnitDoc: modello intermedio comune ai parser YAML/JSON, e builder
//! UnitDoc → Unit|Timer (builder nei task successivi).
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Str(String),
    Bool(bool),
    List(Vec<String>),
}

#[derive(Clone, Debug, Default)]
pub struct UnitDoc(pub Vec<(String, Val)>);

impl UnitDoc {
    pub fn get(&self, key: &str) -> Option<&Val> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn str_of(&self, key: &str) -> Option<&str> {
        match self.get(key) { Some(Val::Str(s)) => Some(s.as_str()), _ => None }
    }
    pub fn bool_of(&self, key: &str) -> Option<bool> {
        match self.get(key) { Some(Val::Bool(b)) => Some(*b), _ => None }
    }
    pub fn list_of(&self, key: &str) -> Option<&[String]> {
        match self.get(key) { Some(Val::List(l)) => Some(l.as_slice()), _ => None }
    }
}
```

In `mod.rs`: `pub mod unitfile; pub mod yaml; #[cfg(feature = "boot-checks")] pub mod checks;`

- [ ] **Step 2: scrivi il boot-check PRIMA del parser** (`checks.rs`)

```rust
//! Self-test init-units, gated `boot-checks`. Panica su mismatch (il run-test
//! QEMU fallisce visibilmente); su pass logga una riga per gruppo.
use alloc::string::ToString;
use alloc::vec::Vec;

pub fn run() {
    check_yaml();
    crate::binfo!("svc-check", "init-units checks OK");
}

fn check_yaml() {
    use super::unitfile::Val;
    let src = "# commento\nname: sshd\ntype: daemon\nenabled: true\nafter: [net, storage]\n\nexec: /mnt/bin/sshd.wasm\n";
    let doc = super::yaml::parse(src).expect("yaml parse");
    assert_eq!(doc.str_of("name"), Some("sshd"));
    assert_eq!(doc.str_of("type"), Some("daemon"));
    assert_eq!(doc.bool_of("enabled"), Some(true));
    assert_eq!(doc.list_of("after"),
        Some(&["net".to_string(), "storage".to_string()][..]));
    assert_eq!(doc.str_of("exec"), Some("/mnt/bin/sshd.wasm"));
    // riga malformata (niente ':') → errore, non panic
    assert!(super::yaml::parse("solo testo\n").is_err());
    crate::binfo!("svc-check", "yaml OK");
}
```

Hook in `boot/phases/userland.rs`, subito dopo `crate::service::init();`:

```rust
#[cfg(feature = "boot-checks")]
crate::service::checks::run();
```

- [ ] **Step 3: cargo check → atteso FAIL** (E0425: `yaml::parse` non esiste). Questo è il red.

- [ ] **Step 4: implementa `yaml.rs`**

```rust
//! Parser YAML-subset line-based: `key: value`, liste inline `[a, b]`,
//! commenti `#`, righe vuote. Niente nesting, niente multiline. Vedi spec
//! init-units §2.
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use super::unitfile::{UnitDoc, Val};

pub fn parse(src: &str) -> Result<UnitDoc, String> {
    let mut doc = UnitDoc::default();
    for (i, raw) in src.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let (key, val) = line.split_once(':')
            .ok_or_else(|| alloc::format!("line {}: missing ':'", i + 1))?;
        let key = key.trim().to_string();
        let val = strip_comment(val).trim();
        doc.0.push((key, parse_val(val)));
    }
    Ok(doc)
}

/// Taglia un commento ` # ...` fuori da una stringa quotata (subset: non
/// supportiamo '#' dentro valori non quotati).
fn strip_comment(v: &str) -> &str {
    match v.find('#') { Some(i) => &v[..i], None => v }
}

fn parse_val(v: &str) -> Val {
    if v == "true"  { return Val::Bool(true); }
    if v == "false" { return Val::Bool(false); }
    if let Some(inner) = v.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let items = inner.split(',')
            .map(|s| unquote(s.trim()).to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        return Val::List(items);
    }
    Val::Str(unquote(v).to_string())
}

fn unquote(v: &str) -> &str {
    let v = v.trim();
    v.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
        .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(v)
}
```

- [ ] **Step 5: cargo check → atteso PASS** (green a livello di compilazione; semantica verificata in QEMU a fine fase parser, Task 5 Step finale).

- [ ] **Step 6: changelog + commit**

Entry `CHANGELOG/430-26-06-11-init-units-yaml-parser.md` (stesso formato del 429: Cosa = UnitDoc/Val + parser YAML-subset + primo boot-check `svc-check`; File toccati = unitfile.rs, yaml.rs, checks.rs, mod.rs, userland.rs).

```bash
git add kernel/src/service/ kernel/src/boot/phases/userland.rs CHANGELOG/430-*.md
git commit -m "feat(init): UnitDoc + parser YAML-subset + boot-check"
```

---

### Task 3: parser JSON-subset

**Files:**
- Create: `kernel/src/service/json.rs`
- Modify: `kernel/src/service/checks.rs`, `kernel/src/service/mod.rs` (+`pub mod json;`)

- [ ] **Step 1: boot-check prima** (aggiungere in `checks.rs::run()` la chiamata `check_json();` e):

```rust
fn check_json() {
    use super::unitfile::Val;
    let src = r#"{ "name":"sshd", "type":"daemon", "enabled":true,
                   "after":["net","storage"], "exec":"/mnt/bin/sshd.wasm" }"#;
    let doc = super::json::parse(src).expect("json parse");
    assert_eq!(doc.str_of("name"), Some("sshd"));
    assert_eq!(doc.bool_of("enabled"), Some(true));
    assert_eq!(doc.list_of("after"),
        Some(&["net".to_string(), "storage".to_string()][..]));
    assert!(super::json::parse("{ broken").is_err());
    crate::binfo!("svc-check", "json OK");
}
```

- [ ] **Step 2: cargo check → FAIL (E0425 `json::parse`)**

- [ ] **Step 3: implementa `json.rs`**

```rust
//! Parser JSON-subset: UN oggetto piatto { "k": v }, v ∈ stringa | bool |
//! numero (tenuto come stringa) | array di stringhe. Char-scanner, zero dep.
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use super::unitfile::{UnitDoc, Val};

struct P<'a> { b: &'a [u8], i: usize }

pub fn parse(src: &str) -> Result<UnitDoc, String> {
    let mut p = P { b: src.as_bytes(), i: 0 };
    p.ws();
    p.expect(b'{')?;
    let mut doc = UnitDoc::default();
    p.ws();
    if p.peek() == Some(b'}') { p.i += 1; return Ok(doc); }
    loop {
        p.ws();
        let key = p.string()?;
        p.ws();
        p.expect(b':')?;
        p.ws();
        let val = p.value()?;
        doc.0.push((key, val));
        p.ws();
        match p.next() {
            Some(b',') => continue,
            Some(b'}') => return Ok(doc),
            _ => return Err("expected ',' or '}'".to_string()),
        }
    }
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<u8> { self.b.get(self.i).copied() }
    fn next(&mut self) -> Option<u8> { let c = self.peek(); if c.is_some() { self.i += 1; } c }
    fn ws(&mut self) { while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) { self.i += 1; } }
    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.next() == Some(c) { Ok(()) } else { Err(alloc::format!("expected '{}'", c as char)) }
    }
    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let start = self.i;
        while let Some(c) = self.peek() {
            if c == b'"' {
                let s = core::str::from_utf8(&self.b[start..self.i])
                    .map_err(|_| "bad utf8".to_string())?.to_string();
                self.i += 1;
                return Ok(s);
            }
            if c == b'\\' { return Err("escape not supported".to_string()); }
            self.i += 1;
        }
        Err("unterminated string".to_string())
    }
    fn value(&mut self) -> Result<Val, String> {
        match self.peek() {
            Some(b'"') => Ok(Val::Str(self.string()?)),
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.peek() == Some(b']') { self.i += 1; return Ok(Val::List(items)); }
                loop {
                    self.ws();
                    items.push(self.string()?);
                    self.ws();
                    match self.next() {
                        Some(b',') => continue,
                        Some(b']') => return Ok(Val::List(items)),
                        _ => return Err("expected ',' or ']'".to_string()),
                    }
                }
            }
            Some(b't') if self.b[self.i..].starts_with(b"true")  => { self.i += 4; Ok(Val::Bool(true)) }
            Some(b'f') if self.b[self.i..].starts_with(b"false") => { self.i += 5; Ok(Val::Bool(false)) }
            Some(c) if c == b'-' || c.is_ascii_digit() => {
                let start = self.i;
                while matches!(self.peek(), Some(c) if c == b'-' || c == b'.' || c.is_ascii_digit()) { self.i += 1; }
                Ok(Val::Str(core::str::from_utf8(&self.b[start..self.i]).unwrap_or("0").to_string()))
            }
            _ => Err("unexpected value".to_string()),
        }
    }
}
```

- [ ] **Step 4: cargo check → PASS**

- [ ] **Step 5: changelog `431-26-06-11-init-units-json-parser.md` + commit**

```bash
git add kernel/src/service/ CHANGELOG/431-*.md
git commit -m "feat(init): parser JSON-subset + boot-check"
```

---

### Task 4: `schedule_parse` + `backoff_ticks`

**Files:**
- Modify: `kernel/src/service/schedule.rs`, `kernel/src/service/checks.rs`

- [ ] **Step 1: boot-check prima** (in `checks.rs::run()` aggiungere `check_schedule();`):

```rust
fn check_schedule() {
    use super::schedule::{schedule_parse, backoff_ticks, Schedule};
    assert_eq!(schedule_parse("daily 03:00"),    Ok(Schedule::Daily { hour: 3, minute: 0 }));
    assert_eq!(schedule_parse("every 300s"),     Ok(Schedule::EveryTicks(30_000)));
    assert_eq!(schedule_parse("boot+10s"),       Ok(Schedule::BootPlus(1_000)));
    assert_eq!(schedule_parse("hourly :15"),     Ok(Schedule::Hourly { minute: 15 }));
    assert_eq!(schedule_parse("weekly Mon 09:30"), Ok(Schedule::Weekly { dow: 1, hour: 9, minute: 30 }));
    assert!(schedule_parse("daily 25:00").is_err());
    assert!(schedule_parse("garbage").is_err());
    assert_eq!(backoff_ticks(0), 100);   // 1s
    assert_eq!(backoff_ticks(1), 200);   // 2s
    assert_eq!(backoff_ticks(4), 1_600); // 16s
    assert_eq!(backoff_ticks(9), 3_000); // cap 30s
    crate::binfo!("svc-check", "schedule OK");
}
```

- [ ] **Step 2: cargo check → FAIL**

- [ ] **Step 3: implementa in `schedule.rs`** (sotto l'enum del Task 1):

```rust
use alloc::string::ToString;

/// "hourly :MM" | "daily HH:MM" | "weekly Dow HH:MM" | "every Ns" | "boot+Ns"
/// Tick = 10 ms (timer 100 Hz): secondi*100.
pub fn schedule_parse(s: &str) -> Result<Schedule, String> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("every ") {
        return Ok(Schedule::EveryTicks(secs(rest)? * 100));
    }
    if let Some(rest) = s.strip_prefix("boot+") {
        return Ok(Schedule::BootPlus(secs(rest)? * 100));
    }
    if let Some(rest) = s.strip_prefix("hourly ") {
        let m = rest.trim().strip_prefix(':').ok_or_else(|| "hourly: want :MM".to_string())?;
        let minute = num(m, 59)? as u8;
        return Ok(Schedule::Hourly { minute });
    }
    if let Some(rest) = s.strip_prefix("daily ") {
        let (h, m) = hhmm(rest)?;
        return Ok(Schedule::Daily { hour: h, minute: m });
    }
    if let Some(rest) = s.strip_prefix("weekly ") {
        let (dow_s, hm) = rest.trim().split_once(' ')
            .ok_or_else(|| "weekly: want 'Dow HH:MM'".to_string())?;
        let dow = match dow_s.to_ascii_lowercase().as_str() {
            "sun" => 0, "mon" => 1, "tue" => 2, "wed" => 3,
            "thu" => 4, "fri" => 5, "sat" => 6,
            _ => return Err("weekly: bad day".to_string()),
        };
        let (h, m) = hhmm(hm)?;
        return Ok(Schedule::Weekly { dow, hour: h, minute: m });
    }
    Err(alloc::format!("unknown schedule '{}'", s))
}

fn secs(s: &str) -> Result<u64, String> {
    let s = s.trim().strip_suffix('s').ok_or_else(|| "want Ns".to_string())?;
    s.parse::<u64>().map_err(|_| "bad number".to_string())
}

fn hhmm(s: &str) -> Result<(u8, u8), String> {
    let (h, m) = s.trim().split_once(':').ok_or_else(|| "want HH:MM".to_string())?;
    Ok((num(h, 23)? as u8, num(m, 59)? as u8))
}

fn num(s: &str, max: u32) -> Result<u32, String> {
    let v = s.trim().parse::<u32>().map_err(|_| "bad number".to_string())?;
    if v > max { return Err(alloc::format!("{} out of range", v)); }
    Ok(v)
}

/// Backoff esponenziale capato: 1s,2s,4s,…,30s (in tick @100Hz).
/// `restarts` = numero di restart già fatti (0-based al primo).
pub fn backoff_ticks(restarts: u32) -> u64 {
    core::cmp::min(100u64 << restarts.min(5), 3_000)
}
```

(Nota: `schedule_parse` ritorna `Result<_, String>` quindi serve `PartialEq` su `Schedule` — già derivato al Task 1.)

- [ ] **Step 4: cargo check → PASS**

- [ ] **Step 5: changelog `432-26-06-11-init-units-schedule-parse.md` + commit**

```bash
git add kernel/src/service/ CHANGELOG/432-*.md
git commit -m "feat(init): schedule_parse + backoff_ticks + boot-check"
```

---

### Task 5: builder `UnitDoc → Unit|Timer` + prima verifica QEMU

**Files:**
- Modify: `kernel/src/service/unitfile.rs`, `kernel/src/service/checks.rs`

- [ ] **Step 1: boot-check prima** (aggiungere `check_unitfile();`):

```rust
fn check_unitfile() {
    use super::unitfile::{build, Parsed};
    use super::{UnitKind, RestartPolicy, ActivateTarget};
    let doc = super::yaml::parse(
        "name: sshd\ntype: daemon\nexec: /mnt/bin/sshd.wasm\nrestart: on-failure\ntarget: boot\nenabled: true\nafter: [net]\nrequires: [net]\n"
    ).unwrap();
    match build(&doc, Some("sshd.yaml")).expect("build unit") {
        Parsed::U(u) => {
            assert_eq!(u.name, "sshd");
            assert_eq!(u.kind, UnitKind::Daemon);
            assert_eq!(u.restart, RestartPolicy::OnFailure);
            assert_eq!(u.target, ActivateTarget::Boot);
            assert!(u.enabled);
            assert_eq!(u.after, alloc::vec!["net".to_string()]);
            assert_eq!(u.file.as_deref(), Some("sshd.yaml"));
        }
        _ => panic!("expected unit"),
    }
    let tdoc = super::yaml::parse(
        "name: backup\nkind: timer\nunit: backup-job\nschedule: daily 03:00\nenabled: true\n"
    ).unwrap();
    match build(&tdoc, None).expect("build timer") {
        Parsed::T(t) => {
            assert_eq!(t.unit, "backup-job");
            assert_eq!(t.schedule, super::schedule::Schedule::Daily { hour: 3, minute: 0 });
        }
        _ => panic!("expected timer"),
    }
    // difetti: manca name → Err; manca exec → Err; defaults
    assert!(build(&super::yaml::parse("type: daemon\n").unwrap(), None).is_err());
    assert!(build(&super::yaml::parse("name: x\n").unwrap(), None).is_err());
    match build(&super::yaml::parse("name: x\nexec: /bin/x.wasm\n").unwrap(), None).unwrap() {
        Parsed::U(u) => {
            assert_eq!(u.kind, UnitKind::Oneshot);          // default
            assert_eq!(u.restart, RestartPolicy::No);        // default
            assert_eq!(u.target, ActivateTarget::Manual);    // default
            assert!(!u.enabled);                             // default
        }
        _ => panic!("expected unit"),
    }
    crate::binfo!("svc-check", "unitfile OK");
}
```

- [ ] **Step 2: cargo check → FAIL**

- [ ] **Step 3: implementa `build` in `unitfile.rs`**

```rust
use super::{Unit, Timer, UnitKind, RestartPolicy, ActivateTarget, UnitStatus};
use super::schedule::schedule_parse;
use alloc::string::ToString;

pub enum Parsed { U(Unit), T(Timer) }

/// UnitDoc → Unit | Timer. `kind: timer` discrimina. Chiavi sconosciute:
/// warn e prosegue. Errori (campi mancanti/valori invalidi) → Err(msg).
pub fn build(doc: &UnitDoc, file: Option<&str>) -> Result<Parsed, String> {
    const KNOWN: &[&str] = &["name", "kind", "type", "exec", "restart", "target",
                             "enabled", "after", "requires", "unit", "schedule"];
    for (k, _) in &doc.0 {
        if !KNOWN.contains(&k.as_str()) {
            crate::bwarn!("svc", "unitfile: unknown key '{}' (ignored)", k);
        }
    }
    let name = doc.str_of("name").ok_or_else(|| "missing 'name'".to_string())?.to_string();

    if doc.str_of("kind") == Some("timer") {
        let unit = doc.str_of("unit").ok_or_else(|| "timer: missing 'unit'".to_string())?.to_string();
        let schedule = schedule_parse(
            doc.str_of("schedule").ok_or_else(|| "timer: missing 'schedule'".to_string())?
        )?;
        return Ok(Parsed::T(Timer {
            name, unit, schedule,
            enabled: doc.bool_of("enabled").unwrap_or(false),
            next_fire: 0,            // armato da load_from_disk/insert
            last_fire: None,
            file: file.map(|s| s.to_string()),
        }));
    }

    let path = doc.str_of("exec").ok_or_else(|| "missing 'exec'".to_string())?.to_string();
    let kind = match doc.str_of("type").unwrap_or("oneshot") {
        "oneshot" => UnitKind::Oneshot,
        "daemon"  => UnitKind::Daemon,
        other => return Err(alloc::format!("bad type '{}'", other)),
    };
    let restart = match doc.str_of("restart").unwrap_or("no") {
        "no"         => RestartPolicy::No,
        "on-failure" => RestartPolicy::OnFailure,
        "always"     => RestartPolicy::Always,
        other => return Err(alloc::format!("bad restart '{}'", other)),
    };
    let target = match doc.str_of("target").unwrap_or("manual") {
        "boot"      => ActivateTarget::Boot,
        "post-boot" => ActivateTarget::PostBoot,
        "manual"    => ActivateTarget::Manual,
        other => return Err(alloc::format!("bad target '{}'", other)),
    };
    let to_vec = |k: &str| doc.list_of(k).map(|l| l.to_vec()).unwrap_or_default();
    Ok(Parsed::U(Unit {
        name, path, kind, restart,
        after: to_vec("after"), requires: to_vec("requires"),
        target,
        enabled: doc.bool_of("enabled").unwrap_or(false),
        status: UnitStatus::Idle, pid: None, runs: 0, restarts: 0,
        stop_requested: false,
        file: file.map(|s| s.to_string()),
    }))
}
```

- [ ] **Step 4: cargo check → PASS**

- [ ] **Step 5: verifica semantica QEMU (chiude la fase parser)**

```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'export PATH=$HOME/.cargo/bin:$PATH; cd /mnt/w/Work/GitHub/ruos && make iso CARGO_FEATURES=boot-checks && make run-test'
```

Expected: PASS; nel log seriale compaiono `svc-check: yaml OK`, `json OK`, `schedule OK`, `unitfile OK`, `init-units checks OK`, nessun panic.

- [ ] **Step 6: changelog `433-26-06-11-init-units-unitfile-builder.md` + commit**

```bash
git add kernel/src/service/ CHANGELOG/433-*.md
git commit -m "feat(init): builder UnitDoc->Unit|Timer + boot-checks verdi in QEMU"
```

---

### Task 6: `compute_next` (calendario)

**Files:**
- Modify: `kernel/src/service/schedule.rs`, `kernel/src/service/checks.rs`

**Nota design (deviazione dichiarata dalla spec):** la spec inietta `now: RtcTime`; qui `compute_next` lavora direttamente su **epoch u64** (`rtc::to_unix_epoch` esiste già) — stessa testabilità, niente ri-implementazione del calendario Gregoriano: rollover giorno/mese/anno sono gestiti dall'aritmetica su epoch. Day-of-week da epoch: `(days + 4) % 7` (1970-01-01 = giovedì = 4).

- [ ] **Step 1: boot-check prima** (`check_compute_next();`):

```rust
fn check_compute_next() {
    use super::schedule::{compute_next, Schedule};
    // 2026-06-08 14:30:00 UTC = 1780929000 (lunedì, dow=1) — verificato con
    // `[DateTimeOffset]::FromUnixTimeSeconds` / `date -u -d @N`.
    let now: u64 = 1_780_929_000;
    assert_eq!(compute_next(&Schedule::Daily { hour: 3, minute: 0 }, now, 0),
               1_780_974_000);              // 2026-06-09 03:00:00 (domani)
    assert_eq!(compute_next(&Schedule::Hourly { minute: 0 }, now, 0),
               1_780_930_800);              // 15:00:00
    assert_eq!(compute_next(&Schedule::Hourly { minute: 45 }, now, 0),
               1_780_929_900);              // 14:45:00 (stessa ora, futuro)
    // weekly Tue 14:00 → domani: 2026-06-09 14:00:00
    assert_eq!(compute_next(&Schedule::Weekly { dow: 2, hour: 14, minute: 0 }, now, 0),
               1_781_013_600);
    // weekly Mon 09:30 → oggi è lunedì ma orario passato → +7g: 2026-06-15 09:30:00
    assert_eq!(compute_next(&Schedule::Weekly { dow: 1, hour: 9, minute: 30 }, now, 0),
               1_781_515_800);
    // rollover anno: 2026-12-31 23:59:00 = 1_798_761_540 → daily 00:00 = 1_798_761_600
    assert_eq!(compute_next(&Schedule::Daily { hour: 0, minute: 0 }, 1_798_761_540, 0),
               1_798_761_600);
    // monotoni: in tick
    assert_eq!(compute_next(&Schedule::EveryTicks(500), now, 12_345), 12_845);
    assert_eq!(compute_next(&Schedule::BootPlus(1_000), now, 12_345), 1_000);
    crate::binfo!("svc-check", "compute_next OK");
}
```

(I valori epoch sono pre-calcolati: 1780929000 = 9 giu 2026 14:30 UTC; verificarli con `date -u -d @N` in WSL durante l'implementazione.)

- [ ] **Step 2: cargo check → FAIL**

- [ ] **Step 3: implementa**

```rust
/// Prossimo scatto FUTURO. Calendario: input/output = unix epoch (s).
/// Monotoni: input/output = tick (EveryTicks→now_ticks+n; BootPlus→n, armato
/// una volta sola al load). Pura: testabile nei boot-check.
pub fn compute_next(s: &Schedule, epoch_now: u64, now_ticks: u64) -> u64 {
    const HOUR: u64 = 3_600;
    const DAY:  u64 = 86_400;
    match *s {
        Schedule::EveryTicks(n) => now_ticks + n,
        Schedule::BootPlus(n)   => n,
        Schedule::Hourly { minute } => {
            let cand = epoch_now - epoch_now % HOUR + u64::from(minute) * 60;
            if cand > epoch_now { cand } else { cand + HOUR }
        }
        Schedule::Daily { hour, minute } => {
            let cand = epoch_now - epoch_now % DAY
                     + u64::from(hour) * HOUR + u64::from(minute) * 60;
            if cand > epoch_now { cand } else { cand + DAY }
        }
        Schedule::Weekly { dow, hour, minute } => {
            let days = epoch_now / DAY;
            let dow_now = ((days + 4) % 7) as u8;          // 1970-01-01 = Thu = 4
            let delta = u64::from((dow + 7 - dow_now) % 7);
            let cand = (days + delta) * DAY
                     + u64::from(hour) * HOUR + u64::from(minute) * 60;
            if cand > epoch_now { cand } else { cand + 7 * DAY }
        }
    }
}
```

- [ ] **Step 4: cargo check → PASS**

- [ ] **Step 5: changelog `434-26-06-11-init-units-compute-next.md` + commit**

```bash
git add kernel/src/service/ CHANGELOG/434-*.md
git commit -m "feat(init): compute_next calendario su epoch + boot-check rollover"
```

---

### Task 7: supervisor — runner pool daemon, restart/backoff, stop

**Files:**
- Modify: `kernel/src/wasm/fiber.rs` (factor `exec_cwasm_inner`)
- Modify: `kernel/src/executor/mod.rs` (runner pool + routing dispatcher)
- Modify: `kernel/src/service/mod.rs` (`MAX_DAEMONS`)

- [ ] **Step 1: factor `exec_cwasm_inner` in `fiber.rs`**

`exec_cwasm_parallel` oggi registra il pid internamente; il runner deve possedere il pid (per `request_kill`). Estrarre il corpo (dalla lettura bytes esclusa, dal pick core alla reply) in:

```rust
/// Esegue bytes .cwasm già letti, con pid già registrato dal chiamante.
/// Stessa logica di exec_cwasm_parallel ma senza register/unregister.
pub async fn exec_cwasm_inner(
    bytes:    alloc::vec::Vec<u8>,
    argv:     alloc::vec::Vec<alloc::vec::Vec<u8>>,
    term_pts: usize,
) -> i32 {
    match crate::executor::pick_compute_core() {
        Some(core) => {
            let reply = crate::executor::ExecReply::new();
            let boxed = bytes.into_boxed_slice();
            match crate::executor::spawn_on(
                core,
                crate::executor::run_app_on_core(boxed, argv, term_pts, reply.clone()),
            ) {
                Ok(()) => crate::executor::ExecReplyFuture(reply).await,
                Err(_) => {
                    crate::bwarn!("exec-ap", "spawn_on({}) failed — pool busy", core);
                    reply.complete(127);
                    127
                }
            }
        }
        None => crate::wasm::wt::run_cwasm(&bytes, argv, Some(term_pts)),
    }
}
```

e `exec_cwasm_parallel` diventa: read_all → `proc::register` → `exec_cwasm_inner(bytes, argv, term_pts).await` → `proc::unregister` → code (comportamento invariato per la shell).

- [ ] **Step 2: runner pool in `executor/mod.rs`** (accanto a `service_dispatcher_task`):

```rust
/// Max daemon supervisionati in parallelo (dimensione del pool di runner task).
pub const MAX_DAEMONS: usize = 8;

/// Runner di un'unità supervisionata: esegue il child, applica la restart
/// policy con backoff esponenziale, esce su stop_requested o policy esaurita.
/// I daemon non hanno PTY: stdout → console (pts 0). Documentato in spec.
#[embassy_executor::task(pool_size = 8)] // = MAX_DAEMONS (l'attr vuole un letterale)
async fn unit_runner_task(name: alloc::string::String) {
    use crate::service::{self, RestartPolicy, UnitStatus};
    loop {
        let Some((_kind, policy, path)) = service::exec_info_of(&name) else { return; };
        let pid = crate::proc::register(name.clone());
        service::mark_running(&name, pid);
        let started = crate::timer::ticks();
        crate::binfo!("svc", "runner start name={} pid={} path={}", name, pid, path);

        let code = if path.ends_with(".cwasm") {
            match crate::wasm::read_all(&path).await {
                Ok(bytes) => crate::wasm::fiber::exec_cwasm_inner(
                    bytes, alloc::vec![name.as_bytes().to_vec()], 0).await,
                Err(_) => { service::mark_failed(&name, "read"); crate::proc::unregister(pid); return; }
            }
        } else {
            match crate::wasm::read_all(&path).await {
                Ok(bytes) => match crate::wasm::fiber::Fiber::new(&bytes) {
                    Ok(mut fb) => {
                        fb.set_args(alloc::vec![name.as_bytes().to_vec()]);
                        fb.set_pid(pid);
                        fb.run().await
                    }
                    Err(_) => { service::mark_failed(&name, "instantiate"); crate::proc::unregister(pid); return; }
                },
                Err(_) => { service::mark_failed(&name, "read"); crate::proc::unregister(pid); return; }
            }
        };
        crate::proc::unregister(pid);
        crate::binfo!("svc", "runner exit name={} code={}", name, code);

        // Uptime > 60s → crash transitorio recuperato: reset del backoff.
        if crate::timer::ticks().saturating_sub(started) > 6_000 {
            service::reset_restarts(&name);
        }
        if service::take_stop_requested(&name) {
            service::mark_exited(&name, code);
            return;
        }
        let restart = matches!(policy, RestartPolicy::Always)
            || (matches!(policy, RestartPolicy::OnFailure) && code != 0);
        if !restart {
            service::mark_exited(&name, code);
            return;
        }
        service::mark_restarting(&name);
        let n = service::bump_restarts(&name);
        let wait = crate::service::schedule::backoff_ticks(n.saturating_sub(1));
        crate::binfo!("svc", "restart name={} #{} in {} ticks", name, n, wait);
        delay::Delay::ticks(wait).await;
        let _ = service::mark_status(&name, UnitStatus::Idle);
    }
}
```

Aggiungere in `service/mod.rs` i due helper mancanti:

```rust
pub fn reset_restarts(name: &str) {
    let mut r = UNITS.lock();
    if let Some(s) = r.iter_mut().find(|s| s.name == name) { s.restarts = 0; }
}

pub fn mark_status(name: &str, st: UnitStatus) -> bool {
    let mut r = UNITS.lock();
    match r.iter_mut().find(|s| s.name == name) {
        Some(s) => { s.status = st; true }
        None => false,
    }
}
```

- [ ] **Step 3: routing nel dispatcher**

In `service_dispatcher_task`, il ramo `Start(name)`: se l'unità è `Daemon` **o** ha `restart != No` → spawn del runner; altrimenti path inline oneshot esistente:

```rust
let (kind, policy, _path) = match crate::service::exec_info_of(&name) {
    Some(t) => t,
    None => { crate::bwarn!("svc", "dispatcher: unknown name '{}'", name); continue; }
};
let supervised = matches!(kind, crate::service::UnitKind::Daemon)
    || !matches!(policy, crate::service::RestartPolicy::No);
if supervised {
    // pool pieno → NoSlot: l'unit resta Idle, log d'errore.
    if crate::executor::spawn_self(unit_runner_task(name.clone())).is_err() {
        crate::bwarn!("svc", "start {}: no free daemon slot (max {})", name, MAX_DAEMONS);
        crate::service::mark_failed(&name, "noslot");
    }
    continue;
}
// …path inline oneshot invariato (Task 1 Step 3)…
```

`spawn_self`: il dispatcher gira sul BSP; usare lo spawner del core 0. Se non esiste già un helper, aggiungerlo in `executor/mod.rs` accanto a `spawn_on` (stesso meccanismo, core fisso 0 — copiare la firma di `spawn_on` che già esiste e delega al `SendSpawner` pubblicato in `PER_CORE_SPAWNER[0]`):

```rust
/// Spawn sul BSP (core 0) dal BSP stesso. Errore = pool del task esaurito.
pub fn spawn_self(token: embassy_executor::SpawnToken<impl Sized>) -> Result<(), embassy_executor::SpawnError> {
    let sp = PER_CORE_SPAWNER[0].lock().clone();
    match sp {
        Some(s) => s.spawn(token),
        None => Err(embassy_executor::SpawnError::Busy),
    }
}
```

(Verificare il nome/tipo esatto del registro spawner: `PER_CORE_SPAWNER[cpu].lock() = Some(spawner.make_send())` in `run_core` — riusarlo.)

- [ ] **Step 4: cargo check → PASS atteso.** Sistemare visibilità (`pub(crate)` su `read_all` già presente in `wasm/mod.rs`; `exec_cwasm_inner` `pub`).

- [ ] **Step 5: changelog `435-26-06-11-init-units-supervisor-runner.md` + commit**

```bash
git add kernel/src/service/ kernel/src/executor/mod.rs kernel/src/wasm/fiber.rs CHANGELOG/435-*.md
git commit -m "feat(init): runner pool daemon con restart/backoff + stop cooperativo"
```

---

### Task 8: scheduler task (timer, polling 1s)

**Files:**
- Modify: `kernel/src/executor/mod.rs` (nuovo task + spawn)
- Modify: `kernel/src/service/mod.rs` (accessor TIMERS)

- [ ] **Step 1: accessor in `service/mod.rs`**

```rust
/// Snapshot dei timer enabled (per lo scheduler): (idx, schedule, next_fire).
pub fn timers_due_snapshot() -> Vec<(usize, schedule::Schedule, u64)> {
    TIMERS.lock().iter().enumerate()
        .filter(|(_, t)| t.enabled)
        .map(|(i, t)| (i, t.schedule.clone(), t.next_fire))
        .collect()
}

/// Registra lo scatto del timer `idx`: last_fire=now, next_fire ricalcolato
/// (futuro); BootPlus si disabilita (one-shot). Ritorna il nome dell'unit.
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
```

- [ ] **Step 2: task in `executor/mod.rs`**

```rust
/// Scheduler dei timer unit: polling 1s (robusto a drift/cambi RTC), "fire if
/// due, recompute to future" → niente doppio scatto, niente backfill.
#[embassy_executor::task]
async fn unit_scheduler_task() {
    let _ = crate::proc::register_kernel("unit-sched");
    loop {
        delay::Delay::ticks(100).await; // ~1s @100Hz
        let ticks = crate::timer::ticks();
        let epoch = crate::rtc::to_unix_epoch(&crate::rtc::now());
        for (idx, sched, next_fire) in crate::service::timers_due_snapshot() {
            let due = match sched {
                crate::service::schedule::Schedule::EveryTicks(_)
                | crate::service::schedule::Schedule::BootPlus(_) => ticks >= next_fire,
                _ => epoch >= next_fire,
            };
            if !due { continue; }
            if let Some(unit) = crate::service::timer_fired(idx, epoch, ticks) {
                crate::binfo!("svc", "timer fired → start {}", unit);
                if let Err(e) = crate::service::start(&unit) {
                    crate::bwarn!("svc", "timer start {}: {}", unit, e);
                }
            }
        }
    }
}
```

Spawn in `run_core(0)` accanto a `service_dispatcher_task` (`kernel/src/executor/mod.rs:254` circa):

```rust
spawner.spawn(unit_scheduler_task()).unwrap();
```

(Verificare il path del modulo rtc: `crate::rtc::now()` / `to_unix_epoch` — `kernel/src/rtc.rs`.)

- [ ] **Step 3: cargo check → PASS**

- [ ] **Step 4: changelog `436-26-06-11-init-units-scheduler.md` + commit**

```bash
git add kernel/src/service/ kernel/src/executor/mod.rs CHANGELOG/436-*.md
git commit -m "feat(init): scheduler timer polling 1s (monotono + calendario RTC)"
```

---

### Task 9: topo-sort (Kahn)

**Files:**
- Create: `kernel/src/service/topo.rs`
- Modify: `kernel/src/service/mod.rs` (+`pub mod topo;`), `kernel/src/service/checks.rs`

- [ ] **Step 1: boot-check prima** (`check_topo();`):

```rust
fn check_topo() {
    use super::topo::topo_sort;
    use alloc::vec;
    // A after B → ordine [B, A]
    let (order, cyc) = topo_sort(&[
        ("a".to_string(), vec!["b".to_string()]),
        ("b".to_string(), vec![]),
    ]);
    assert_eq!(order, vec!["b".to_string(), "a".to_string()]);
    assert!(cyc.is_empty());
    // catena transitiva c→b→a
    let (order, _) = topo_sort(&[
        ("c".to_string(), vec!["b".to_string()]),
        ("a".to_string(), vec![]),
        ("b".to_string(), vec!["a".to_string()]),
    ]);
    assert_eq!(order, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    // ciclo a↔b rilevato; c indipendente prosegue
    let (order, cyc) = topo_sort(&[
        ("a".to_string(), vec!["b".to_string()]),
        ("b".to_string(), vec!["a".to_string()]),
        ("c".to_string(), vec![]),
    ]);
    assert_eq!(order, vec!["c".to_string()]);
    assert_eq!(cyc.len(), 2);
    // dep esterna al set (es. builtin già Running) → ignorata, non blocca
    let (order, cyc) = topo_sort(&[("a".to_string(), vec!["ssh".to_string()])]);
    assert_eq!(order, vec!["a".to_string()]);
    assert!(cyc.is_empty());
    crate::binfo!("svc-check", "topo OK");
}
```

- [ ] **Step 2: cargo check → FAIL**

- [ ] **Step 3: implementa `topo.rs`**

```rust
//! Topo-sort (Kahn) sul grafo after ∪ requires, ristretto al set di nodi
//! passato. Dipendenze verso nomi fuori dal set sono ignorate (es. builtin
//! già attivi). Ritorna (ordine, nodi_in_ciclo).
use alloc::string::String;
use alloc::vec::Vec;

pub fn topo_sort(nodes: &[(String, Vec<String>)]) -> (Vec<String>, Vec<String>) {
    let in_set = |n: &str| nodes.iter().any(|(name, _)| name == n);
    // indegree = numero di dep DENTRO il set
    let mut indeg: Vec<usize> = nodes.iter()
        .map(|(_, deps)| deps.iter().filter(|d| in_set(d)).count())
        .collect();
    let mut order = Vec::with_capacity(nodes.len());
    let mut done = alloc::vec![false; nodes.len()];
    loop {
        // il più piccolo indice con indegree 0 non ancora emesso (deterministico)
        let Some(i) = (0..nodes.len()).find(|&i| !done[i] && indeg[i] == 0) else { break; };
        done[i] = true;
        order.push(nodes[i].0.clone());
        for (j, (_, deps)) in nodes.iter().enumerate() {
            if !done[j] && deps.iter().any(|d| *d == nodes[i].0) {
                indeg[j] = indeg[j].saturating_sub(1);
            }
        }
    }
    let cyclic = nodes.iter().enumerate()
        .filter(|(i, _)| !done[*i])
        .map(|(_, (n, _))| n.clone())
        .collect();
    (order, cyclic)
}
```

- [ ] **Step 4: cargo check → PASS**

- [ ] **Step 5: changelog `437-26-06-11-init-units-toposort.md` + commit**

```bash
git add kernel/src/service/ CHANGELOG/437-*.md
git commit -m "feat(init): topo-sort Kahn con rilevamento cicli + boot-check"
```

---

### Task 10: `activate_target` + hook di boot

**Files:**
- Modify: `kernel/src/service/mod.rs` (activate_target, is_up, requires-closure)
- Modify: `kernel/src/executor/mod.rs` (`init_units_task` + spawn)

- [ ] **Step 1: implementa in `service/mod.rs`**

```rust
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
/// prima di avviare le dipendenti. Vedi spec §6.
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

    // 3. avvio in ordine, attesa "su" delle dep (cap 10s), requires fallito → Failed(dep)
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
```

(Verificare la visibilità di `crate::executor::delay::Delay` — se il modulo `delay` non è `pub`, renderlo `pub mod delay;` in `executor/mod.rs`.)

- [ ] **Step 2: `init_units_task` in `executor/mod.rs`**

```rust
/// Orchestrazione boot delle unit: carica i file, attiva Boot, poi PostBoot
/// quando shell/compositor hanno avuto modo di partire. load_from_disk arriva
/// al Task 11 — fino ad allora attiva solo i builtin seedati da service::init.
#[embassy_executor::task]
async fn init_units_task() {
    let _ = crate::proc::register_kernel("init-units");
    crate::service::load_from_disk().await;
    crate::service::activate_target(crate::service::ActivateTarget::Boot).await;
    delay::Delay::ticks(300).await;   // ~3s: shell + compositor su
    crate::service::activate_target(crate::service::ActivateTarget::PostBoot).await;
    crate::binfo!("svc", "unit activation complete");
}
```

Per far compilare PRIMA del Task 11, aggiungere in `service/mod.rs` lo stub:

```rust
/// Carica /mnt/etc/units/*.{yaml,yml,json}. Implementazione al Task 11.
pub async fn load_from_disk() {}
```

Spawn in `run_core(0)`: `spawner.spawn(init_units_task()).unwrap();` (DOPO `service_dispatcher_task` e `unit_scheduler_task`).

- [ ] **Step 3: cargo check → PASS**

- [ ] **Step 4: verifica QEMU intermedia**

`make iso CARGO_FEATURES=boot-checks && make run-test` → PASS, nel log: `svc: unit activation complete`, nessun deadlock di boot (l'attivazione gira nel task, non blocca le fasi).

- [ ] **Step 5: changelog `438-26-06-11-init-units-activate-target.md` + commit**

```bash
git add kernel/src/service/ kernel/src/executor/mod.rs CHANGELOG/438-*.md
git commit -m "feat(init): activate_target topo-ordinato + init_units_task boot/post-boot"
```

---

### Task 11: `load_from_disk`

**Files:**
- Modify: `kernel/src/service/mod.rs`

- [ ] **Step 1: implementa (sostituisce lo stub)**

```rust
pub const UNITS_DIR: &str = "/mnt/etc/units";

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
```

Nota: `compute_next` per `BootPlus(n)` ritorna `n` (tick assoluto da boot) e per `EveryTicks(n)` `ticks+n` — coerente con l'arming richiesto dalla spec.

- [ ] **Step 2: cargo check → PASS**

- [ ] **Step 3: changelog `439-26-06-11-init-units-load-from-disk.md` + commit**

```bash
git add kernel/src/service/mod.rs CHANGELOG/439-*.md
git commit -m "feat(init): load_from_disk /mnt/etc/units (yaml+json, robusto a errori)"
```

---

### Task 12: persistenza enable/disable + reload

**Files:**
- Modify: `kernel/src/service/unitfile.rs` (serializzatori), `kernel/src/service/mod.rs` (enable/persist/reload), `kernel/src/executor/mod.rs` (dispatcher: rami Persist/Reload), `kernel/src/service/checks.rs`

- [ ] **Step 1: boot-check del serializzatore prima** (`check_serialize();`):

```rust
fn check_serialize() {
    use super::unitfile::{build, to_yaml, to_json, Parsed};
    let src = "name: sshd\ntype: daemon\nexec: /mnt/bin/sshd.wasm\nrestart: on-failure\ntarget: boot\nenabled: true\nafter: [net]\nrequires: [net]\n";
    let Parsed::U(u) = build(&super::yaml::parse(src).unwrap(), None).unwrap() else { panic!() };
    // roundtrip YAML: serialize → parse → build → campi uguali
    let y = to_yaml(&u);
    let Parsed::U(u2) = build(&super::yaml::parse(&y).unwrap(), None).unwrap() else { panic!() };
    assert_eq!(u2.name, u.name);
    assert_eq!(u2.enabled, u.enabled);
    assert_eq!(u2.restart, u.restart);
    assert_eq!(u2.after, u.after);
    // roundtrip JSON
    let j = to_json(&u);
    let Parsed::U(u3) = build(&super::json::parse(&j).unwrap(), None).unwrap() else { panic!() };
    assert_eq!(u3.name, u.name);
    assert_eq!(u3.enabled, u.enabled);
    crate::binfo!("svc-check", "serialize OK");
}
```

- [ ] **Step 2: cargo check → FAIL**

- [ ] **Step 3: serializzatori in `unitfile.rs`**

```rust
fn kind_str(k: UnitKind) -> &'static str {
    match k { UnitKind::Oneshot => "oneshot", UnitKind::Daemon => "daemon" }
}
fn restart_str(r: RestartPolicy) -> &'static str {
    match r { RestartPolicy::No => "no", RestartPolicy::OnFailure => "on-failure", RestartPolicy::Always => "always" }
}
fn target_str(t: ActivateTarget) -> &'static str {
    match t { ActivateTarget::Boot => "boot", ActivateTarget::PostBoot => "post-boot", ActivateTarget::Manual => "manual" }
}

pub fn to_yaml(u: &Unit) -> alloc::string::String {
    let mut s = alloc::format!(
        "name: {}\ntype: {}\nexec: {}\nrestart: {}\ntarget: {}\nenabled: {}\n",
        u.name, kind_str(u.kind), u.path, restart_str(u.restart),
        target_str(u.target), u.enabled);
    if !u.after.is_empty()    { s += &alloc::format!("after: [{}]\n",    u.after.join(", ")); }
    if !u.requires.is_empty() { s += &alloc::format!("requires: [{}]\n", u.requires.join(", ")); }
    s
}

pub fn to_json(u: &Unit) -> alloc::string::String {
    let list = |l: &[alloc::string::String]| -> alloc::string::String {
        let items: Vec<alloc::string::String> =
            l.iter().map(|x| alloc::format!("\"{}\"", x)).collect();
        alloc::format!("[{}]", items.join(","))
    };
    alloc::format!(
        "{{ \"name\":\"{}\", \"type\":\"{}\", \"exec\":\"{}\", \"restart\":\"{}\", \"target\":\"{}\", \"enabled\":{}, \"after\":{}, \"requires\":{} }}\n",
        u.name, kind_str(u.kind), u.path, restart_str(u.restart),
        target_str(u.target), u.enabled, list(&u.after), list(&u.requires))
}
```

(`u.after.join` richiede `use alloc::string::String;` e slice di String — `join` è disponibile su `[String]` via alloc.)

- [ ] **Step 4: enable/persist/reload in `service/mod.rs`**

```rust
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

/// Ri-parsa la dir: nuove unit aggiunte, esistenti (file-sourced) aggiornate
/// nei campi config (status runtime preservato), rimosse dal disco → drop se
/// non Running. Timer: ri-armati.
pub async fn reload() {
    // marca le file-sourced viste prima del reload
    let before: Vec<String> = UNITS.lock().iter()
        .filter(|u| u.file.is_some()).map(|u| u.name.clone()).collect();
    let before_t: Vec<String> = TIMERS.lock().iter()
        .filter(|t| t.file.is_some()).map(|t| t.name.clone()).collect();

    // ri-parse della dir su registri temporanei: riusa load_from_disk ma su
    // collezioni locali → qui inline per semplicità: rimuovi e ricarica.
    // 1. drop di tutte le file-sourced non-Running (le Running restano con la
    //    config vecchia fino al prossimo restart — warn).
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
    // 2. ricarica (i duplicati con le Running superstiti vengono skippati con warn)
    load_from_disk().await;
    let after = UNITS.lock().iter().filter(|u| u.file.is_some()).count();
    crate::binfo!("svc", "reload done (before {} units/{} timers, now {} file units)",
        before.len(), before_t.len(), after);
}
```

- [ ] **Step 5: rami dispatcher** (sostituire il `bwarn` placeholder del Task 1):

```rust
crate::service::UnitReq::Persist(n) => { crate::service::persist(&n).await; continue; }
crate::service::UnitReq::Reload     => { crate::service::reload().await;    continue; }
```

- [ ] **Step 6: cargo check → PASS; poi QEMU** `make iso CARGO_FEATURES=boot-checks && make run-test` → PASS con `svc-check: serialize OK`.

- [ ] **Step 7: changelog `440-26-06-11-init-units-persist-reload.md` + commit**

```bash
git add kernel/src/service/ kernel/src/executor/mod.rs CHANGELOG/440-*.md
git commit -m "feat(init): persistenza enable/disable su file + reload"
```

---

### Task 13: host ABI `ruos.unit_*` + doc API

**Files:**
- Create: `kernel/src/wasm/host/unit.rs`
- Modify: `kernel/src/wasm/host/mod.rs` (`pub mod unit;` + `unit::link(linker)?;` in `install()`)
- Modify: `kernel/src/service/mod.rs` (snapshot estesi)
- Modify: `docs/api/ruos.md` (STESSO commit — regola CLAUDE.md)

- [ ] **Step 1: snapshot estesi in `service/mod.rs`**

```rust
/// Riga TSV estesa per unit_list/unit_status:
/// name\tkind\tstatus\tpid\truns\trestarts\ttarget\tenabled\tpath\tfile\n
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
        UnitStatus::Exited(c)  => alloc::format!("Exited({})", c),
        UnitStatus::Failed(m)  => alloc::format!("Failed({})", m),
        other => other.label().to_string(),
    };
    let _ = write!(out, "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
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

/// name\tunit\tschedule\tenabled\tnext_fire\tlast_fire\n (fire in tick o epoch
/// secondo il tipo di schedule — il tool mostra il raw + tipo).
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
        let _ = write!(out, "{}\t{}\t{}\t{}\t{}\t{}\n",
            t.name, t.unit, sched, t.enabled, t.next_fire,
            t.last_fire.map(|v| alloc::format!("{}", v)).unwrap_or_else(|| "-".to_string()));
    }
    out
}
```

- [ ] **Step 2: `host/unit.rs`** (pattern identico a `host/service.rs` — buffer+used, ENOBUFS=8):

```rust
//! Host fns `ruos.unit_*`: ABI del CLI unitctl. TSV (vedi service::list_tsv).
use alloc::string::String;
use wasmi::{Caller, Linker};
use crate::wasm::{RuntimeState, Error};

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("ruos", "unit_list",   ruos_unit_list)?
        .func_wrap("ruos", "unit_status", ruos_unit_status)?
        .func_wrap("ruos", "unit_start",  ruos_unit_start)?
        .func_wrap("ruos", "unit_stop",   ruos_unit_stop)?
        .func_wrap("ruos", "unit_enable", ruos_unit_enable)?
        .func_wrap("ruos", "timer_list",  ruos_timer_list)?
        .func_wrap("ruos", "unit_reload", ruos_unit_reload)?;
    Ok(())
}

fn write_text(
    caller: &mut Caller<'_, RuntimeState>,
    text: &str, buf_ptr: i32, buf_len: i32, used_ptr: i32,
) -> i32 {
    let bytes = text.as_bytes();
    if let Err(e) = crate::wasm::host::mem::guest_write_u32(caller, used_ptr, bytes.len() as u32) {
        return e;
    }
    if (buf_len as usize) < bytes.len() { return 8; } // ENOBUFS
    if let Err(e) = crate::wasm::host::mem::guest_write(caller, buf_ptr, bytes) { return e; }
    0
}

fn read_name(caller: &Caller<'_, RuntimeState>, ptr: i32, len: i32) -> Result<String, Error> {
    let buf = crate::wasm::host::mem::guest_read(caller, ptr, len)
        .map_err(|_| Error::i32_exit(-1))?;
    core::str::from_utf8(&buf).map(|s| s.into()).map_err(|_| Error::i32_exit(-1))
}

pub fn ruos_unit_list(
    mut caller: Caller<'_, RuntimeState>, buf_ptr: i32, buf_len: i32, used_ptr: i32,
) -> Result<i32, Error> {
    let text = crate::service::list_tsv();
    Ok(write_text(&mut caller, &text, buf_ptr, buf_len, used_ptr))
}

pub fn ruos_unit_status(
    mut caller: Caller<'_, RuntimeState>,
    name_ptr: i32, name_len: i32, buf_ptr: i32, buf_len: i32, used_ptr: i32,
) -> Result<i32, Error> {
    let name = read_name(&caller, name_ptr, name_len)?;
    match crate::service::status_tsv(&name) {
        Some(text) => Ok(write_text(&mut caller, &text, buf_ptr, buf_len, used_ptr)),
        None => Ok(1), // NotFound
    }
}

pub fn ruos_unit_start(
    caller: Caller<'_, RuntimeState>, name_ptr: i32, name_len: i32,
) -> Result<i32, Error> {
    let name = read_name(&caller, name_ptr, name_len)?;
    Ok(match crate::service::start(&name) { Ok(()) => 0, Err(e) => e.errno() })
}

pub fn ruos_unit_stop(
    caller: Caller<'_, RuntimeState>, name_ptr: i32, name_len: i32,
) -> Result<i32, Error> {
    let name = read_name(&caller, name_ptr, name_len)?;
    Ok(match crate::service::stop(&name) { Ok(()) => 0, Err(e) => e.errno() })
}

pub fn ruos_unit_enable(
    caller: Caller<'_, RuntimeState>, name_ptr: i32, name_len: i32, on: i32,
) -> Result<i32, Error> {
    let name = read_name(&caller, name_ptr, name_len)?;
    Ok(match crate::service::set_enabled(&name, on != 0) { Ok(()) => 0, Err(e) => e.errno() })
}

pub fn ruos_timer_list(
    mut caller: Caller<'_, RuntimeState>, buf_ptr: i32, buf_len: i32, used_ptr: i32,
) -> Result<i32, Error> {
    let text = crate::service::timers_tsv();
    Ok(write_text(&mut caller, &text, buf_ptr, buf_len, used_ptr))
}

pub fn ruos_unit_reload(_caller: Caller<'_, RuntimeState>) -> Result<i32, Error> {
    use crate::service::{SERVICE_QUEUE, UnitReq};
    SERVICE_QUEUE.pending.lock().push_back(UnitReq::Reload);
    if let Some(w) = SERVICE_QUEUE.worker_waker.lock().take() { w.wake(); }
    Ok(0)
}
```

(Copiare gli `use`/path esatti da `host/service.rs` se differiscono — es. il tipo `Error` e il modulo `mem`.)

- [ ] **Step 3: wiring** in `host/mod.rs`: `pub mod unit;` + `unit::link(linker)?;` dentro `install()` dopo `service::link(linker)?;`.

- [ ] **Step 4: doc API — `docs/api/ruos.md`** (stesso commit). Aggiungere alla tabella della sezione service (o nuova sottosezione "Unit manager"):

```markdown
| `unit_list(buf, len, used) -> i32` | TSV `name\tkind\tstatus\tpid\truns\trestarts\ttarget\tenabled\tpath\tfile\n`, una riga per unit. `8` ENOBUFS (used = size necessaria). |
| `unit_status(name_ptr, name_len, buf, len, used) -> i32` | Riga TSV della singola unit. `1` NotFound. |
| `unit_start(name_ptr, name_len) -> i32` | Avvia (async via queue). `1` NotFound, `2` Already, `3` NotSupported, `4` NoSlot, `99` Internal. |
| `unit_stop(name_ptr, name_len) -> i32` | Stop cooperativo (request_kill + no-restart). Best-effort. `1` NotFound, `3` NotSupported (builtin). |
| `unit_enable(name_ptr, name_len, on) -> i32` | enabled on/off + riscrittura del file sorgente (persistente). `1` NotFound. |
| `timer_list(buf, len, used) -> i32` | TSV `name\tunit\tschedule\tenabled\tnext_fire\tlast_fire\n`. |
| `unit_reload() -> i32` | Ri-parsa `/mnt/etc/units` (add/update/remove). Sempre `0` (async). |
```

Aggiornare anche la riga "Last reviewed" della pagina.

- [ ] **Step 5: cargo check → PASS**

- [ ] **Step 6: changelog `441-26-06-11-init-units-host-abi.md` + commit**

```bash
git add kernel/src/wasm/host/ kernel/src/service/mod.rs docs/api/ruos.md CHANGELOG/441-*.md
git commit -m "feat(init): host ABI ruos.unit_* + timer_list + doc API"
```

---

### Task 14: tool `unitctl`

**Files:**
- Create: `user/unitctl/Cargo.toml`, `user/unitctl/src/main.rs`
- Modify: `user/Cargo.toml` (member `"unitctl"`), `Makefile` (`unitctl` in `BIN_TOOLS`)

- [ ] **Step 1: `user/unitctl/Cargo.toml`**

```toml
[package]
name = "unitctl"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "unitctl"
path = "src/main.rs"
```

- [ ] **Step 2: `user/unitctl/src/main.rs`** (pattern di `user/service/src/main.rs`):

```rust
//! `unitctl` — init system CLI (estende `service`). Host fns `ruos.unit_*`.
//!
//! unitctl [list] | status <n> | start <n> | stop <n> | enable <n> |
//!         disable <n> | timers | reload | cat <n>
//!
//! Exit: 0 ok; 1 errore (errno != 0); 2 usage.

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn unit_list(buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
    fn unit_status(name_ptr: u32, name_len: u32, buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
    fn unit_start(name_ptr: u32, name_len: u32) -> i32;
    fn unit_stop(name_ptr: u32, name_len: u32) -> i32;
    fn unit_enable(name_ptr: u32, name_len: u32, on: i32) -> i32;
    fn timer_list(buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
    fn unit_reload() -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sub = args.first().map(String::as_str).unwrap_or("list");
    let name = || -> &str {
        match args.get(1) { Some(n) => n, None => usage_err("missing unit name") }
    };
    match sub {
        "list"    => cmd_list(),
        "status"  => cmd_status(name()),
        "start"   => cmd_errno("start",   unsafe_call1(name(), |p, l| unsafe { unit_start(p, l) })),
        "stop"    => cmd_errno("stop",    unsafe_call1(name(), |p, l| unsafe { unit_stop(p, l) })),
        "enable"  => cmd_errno("enable",  unsafe_call1(name(), |p, l| unsafe { unit_enable(p, l, 1) })),
        "disable" => cmd_errno("disable", unsafe_call1(name(), |p, l| unsafe { unit_enable(p, l, 0) })),
        "timers"  => cmd_timers(),
        "reload"  => cmd_errno("reload",  unsafe { unit_reload() }),
        "cat"     => cmd_cat(name()),
        other => usage_err(&format!("unknown subcommand: {}", other)),
    }
}

fn unsafe_call1(name: &str, f: impl Fn(u32, u32) -> i32) -> i32 {
    let b = name.as_bytes();
    f(b.as_ptr() as u32, b.len() as u32)
}

fn fetch(f: impl Fn(u32, u32, u32) -> i32) -> Option<String> {
    let mut buf = vec![0u8; 16384];
    let mut used: u32 = 0;
    let errno = f(buf.as_mut_ptr() as u32, buf.len() as u32, &mut used as *mut u32 as u32);
    if errno != 0 { eprintln!("unitctl: errno {}", errno); return None; }
    let n = (used as usize).min(buf.len());
    Some(String::from_utf8_lossy(&buf[..n]).into_owned())
}

fn cmd_list() {
    let Some(text) = fetch(|b, l, u| unsafe { unit_list(b, l, u) }) else { std::process::exit(1) };
    println!("{:<14} {:<8} {:<14} {:>5} {:>4} {:>4}  {:<9} {:<3}  {}",
        "NAME", "KIND", "STATUS", "PID", "RUNS", "RST", "TARGET", "EN", "PATH");
    for line in text.lines().filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 10 { continue; }
        let en = if f[7] == "true" { "on" } else { "off" };
        println!("{:<14} {:<8} {:<14} {:>5} {:>4} {:>4}  {:<9} {:<3}  {}",
            f[0], f[1], f[2], f[3], f[4], f[5], f[6], en, f[8]);
    }
}

fn cmd_status(name: &str) {
    let b = name.as_bytes();
    let Some(text) = fetch(|buf, l, u| unsafe {
        unit_status(b.as_ptr() as u32, b.len() as u32, buf, l, u)
    }) else { std::process::exit(1) };
    let f: Vec<&str> = text.trim_end().split('\t').collect();
    if f.len() < 10 { eprintln!("unitctl: bad record"); std::process::exit(1); }
    println!("{} kind={} status={} pid={} runs={} restarts={} target={} enabled={}",
        f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7]);
    println!("  exec: {}", f[8]);
    if f[9] != "-" { println!("  file: {}", f[9]); }
}

fn cmd_timers() {
    let Some(text) = fetch(|b, l, u| unsafe { timer_list(b, l, u) }) else { std::process::exit(1) };
    println!("{:<14} {:<14} {:<18} {:<3}  {:>12} {:>12}",
        "NAME", "UNIT", "SCHEDULE", "EN", "NEXT", "LAST");
    for line in text.lines().filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 6 { continue; }
        let en = if f[3] == "true" { "on" } else { "off" };
        println!("{:<14} {:<14} {:<18} {:<3}  {:>12} {:>12}", f[0], f[1], f[2], en, f[4], f[5]);
    }
}

fn cmd_errno(op: &str, errno: i32) {
    match errno {
        0 => println!("unitctl: {}: ok", op),
        1 => { eprintln!("unitctl: {}: not found", op);          std::process::exit(1); }
        2 => { eprintln!("unitctl: {}: already running", op);    std::process::exit(1); }
        3 => { eprintln!("unitctl: {}: not supported", op);      std::process::exit(1); }
        4 => { eprintln!("unitctl: {}: no free daemon slot", op); std::process::exit(1); }
        n => { eprintln!("unitctl: {}: errno {}", op, n);        std::process::exit(1); }
    }
}

fn cmd_cat(name: &str) {
    // colonna 10 (file) dello status → lettura via WASI
    let b = name.as_bytes();
    let Some(text) = fetch(|buf, l, u| unsafe {
        unit_status(b.as_ptr() as u32, b.len() as u32, buf, l, u)
    }) else { std::process::exit(1) };
    let file = text.trim_end().split('\t').nth(9).unwrap_or("-");
    if file == "-" { eprintln!("unitctl: cat: '{}' has no source file", name); std::process::exit(1); }
    match std::fs::read_to_string(file) {
        Ok(s) => print!("{}", s),
        Err(e) => { eprintln!("unitctl: cat {}: {}", file, e); std::process::exit(1); }
    }
}

fn usage_err(msg: &str) -> ! {
    eprintln!("unitctl: {}", msg);
    eprintln!("usage: unitctl [list|status <n>|start <n>|stop <n>|enable <n>|disable <n>|timers|reload|cat <n>]");
    std::process::exit(2);
}
```

(NB: `cmd_errno("start", unsafe_call1(...))` valuta l'argomento prima della chiamata — ok. Se il borrow di `args` nel closure `name()` dà problemi, sostituire con un match esplicito per subcommand come fa `user/service/src/main.rs`.)

- [ ] **Step 3: wiring build**

- `user/Cargo.toml`: aggiungere `"unitctl"` alla lista `members` (riga dopo `"service"`).
- `Makefile`: aggiungere `unitctl` alla variabile `BIN_TOOLS` (riga ~29, accanto a `service`).

- [ ] **Step 4: builda il tool**

```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'export PATH=$HOME/.cargo/bin:$PATH; cd /mnt/w/Work/GitHub/ruos/user && cargo build --target wasm32-wasip1 --release -p unitctl'
```

Expected: `Finished release`.

- [ ] **Step 5: changelog `442-26-06-11-init-units-unitctl.md` + commit**

```bash
git add user/unitctl/ user/Cargo.toml Makefile CHANGELOG/442-*.md
git commit -m "feat(init): tool unitctl (list/status/start/stop/enable/timers/reload/cat)"
```

---

### Task 15: verifica end-to-end + chiusura

- [ ] **Step 1: full test suite**

```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'export PATH=$HOME/.cargo/bin:$PATH; cd /mnt/w/Work/GitHub/ruos && make iso CARGO_FEATURES=boot-checks && make run-test'
```

Expected: PASS + tutti i `svc-check: * OK` nel log seriale.

- [ ] **Step 2: build ISO release (senza checks)**

```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'export PATH=$HOME/.cargo/bin:$PATH; cd /mnt/w/Work/GitHub/ruos && make iso'
```

- [ ] **Step 3: checklist manuale in QEMU (`make run`, serve disco FAT32 montato per /mnt)**

Dalla shell ruos:
1. `mkdir /mnt/etc` + `mkdir /mnt/etc/units`; con `nano` creare `/mnt/etc/units/hello.yaml`:
   `name: hello` / `type: oneshot` / `exec: /bin/whoami.wasm` / `target: manual` / `enabled: true`
   e `/mnt/etc/units/tick.yaml`: `name: tick` / `kind: timer` / `unit: hello` / `schedule: every 10s` / `enabled: true`.
2. `unitctl reload` → `unitctl list` mostra `hello`; `unitctl timers` mostra `tick`.
3. `unitctl start hello` → status passa per Running → `Exited(0)`, `runs` incrementa.
4. Attendere ~10s → il timer scatta da solo (runs incrementa ancora; `timers` mostra last/next aggiornati).
5. `unitctl disable hello` → reboot → `enabled` ancora off (persistenza FAT32).
6. Daemon: unit `type: daemon` + `restart: always` su un tool che esce (es. whoami) → si riavvia con backoff visibile nei log (`svc: restart ... in N ticks`); `unitctl stop` → resta giù.
7. Deps: `b.yaml` con `after: [hello]`, `requires: [hello]`, `target: post-boot`, `enabled: true` → al boot parte dopo hello; con `exec` inesistente su hello → b va `Failed(dep)`.

Annotare gli esiti; ogni difetto → fix con commit dedicato (changelog incluso).

- [ ] **Step 4: aggiorna lo stato della spec**

In `docs/superpowers/specs/2026-06-09-init-units-timers-design.md` aggiungere sotto il titolo:
`**Stato:** implementato (vedi CHANGELOG 429-44x; piano docs/superpowers/plans/2026-06-11-init-units-timers.md)`

- [ ] **Step 5: changelog finale `443-26-06-11-init-units-e2e.md` + commit**

```bash
git add docs/superpowers/specs/2026-06-09-init-units-timers-design.md CHANGELOG/443-*.md
git commit -m "docs(init): spec init-units marcata implementata + esiti verifica e2e"
```

---

## Fuori scope (dalla spec, ribadito)

Niente cron 5-campi, niente backfill scatti persi, niente stop preemptive, niente offload `.wasm` su compute core, niente socket/path activation. `unitctl` NON sostituisce il tool `service` in questo progetto (convive; eventuale deprecazione a parte).

## Rischi noti / decisioni prese nel piano

- **`compute_next` su epoch** invece di `RtcTime` iniettato (deviazione dichiarata, Task 6): meno codice, stessi test.
- **Daemon senza PTY**: stdout → pts 0 (console). Accettato per v1, documentato nel runner.
- **`embassy_executor::task(pool_size = 8)`**: l'attributo richiede un letterale; tenere allineato a `MAX_DAEMONS` a mano (commento sul posto).
- **Reload con unit Running**: config vecchia fino al prossimo restart (warn esplicito) — niente hot-swap.
- **`unit_reload` errno sempre 0**: il parse è async nel dispatcher; gli errori vanno nel klog. Accettato (il CLI dice "ok (async)").
