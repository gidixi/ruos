# Init System (units, timers, supervision) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Un init system nativo ruos (stile systemd+cron): unit con tipo oneshot/daemon, restart policy, dipendenze/ordine, target di attivazione (boot/post-boot/manual), timer interval+calendario (RTC), supervisione daemon, config su file YAML-subset+JSON, CLI `unitctl`.

**Architecture:** Estende il service manager esistente (`service/mod.rs`) in "unit manager" + due task async sul BSP (`supervisor_task`, `scheduler_task`). Esecuzione delegata al routing exec esistente (`.cwasm`→compute core via `exec_cwasm_parallel`, `.wasm`→BSP wasmi). Config parsata da parser hand-written (no crate no_std YAML affidabile) in un modello comune `UnitDoc`. Shared-nothing: registry owned dal BSP, richieste cross-core via `UNIT_QUEUE`+waker. Vedi spec `docs/superpowers/specs/2026-06-09-init-units-timers-design.md`.

**Tech Stack:** Rust `no_std` kernel (target `x86_64-unknown-none`, build-std), embassy-executor async, RTC (`kernel/src/rtc.rs`), wasmi/Wasmtime exec, VFS/FAT32. Userspace tool wasm32-wasip1. **No `cargo test` sul kernel** → verifica per task = compile via WSL + boot-checks (fasi finali) + manuale.

**Build/verify (via WSL):**
- Kernel compile: `wsl -d Ubuntu-22.04 -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/w/Work/GitHub/ruos/kernel && cargo build --release 2>&1 | tail -8'`
- Boot-checks: aggiungi `--features boot-checks`; run: `... cd /mnt/w/Work/GitHub/ruos && make iso CARGO_FEATURES=boot-checks && make run-test 2>&1 | tail -30`
- User tool: `... cd /mnt/w/Work/GitHub/ruos && make user/unitctl` (target `user-wasm`) — copia in `user-bin/`.
- ISO completa: `... make iso`.

**Repo/branch:** `main`, no branch nuovo (autorizzato). Ogni commit padre → entry `CHANGELOG/NN-...`. **Numerazione: parti da 381** (max attuale 380). Working tree CLEAN tra i task → ogni task fa `git add <path espliciti>`, MAI `git add -A`.

---

## File Structure

- `kernel/src/service/mod.rs` — **Modify.** `Unit`/`Timer` + enums; registry `UNITS`/`TIMERS`; `UNIT_QUEUE`; `start/stop/enable/list/status/timers/reload`; `activate_target`; `load_from_disk`.
- `kernel/src/service/unitdoc.rs` — **Create.** Modello comune `UnitDoc` (`Val{Str,Bool,List}` + lookup helpers).
- `kernel/src/service/yaml.rs` — **Create.** Parser YAML-subset → `UnitDoc`.
- `kernel/src/service/json.rs` — **Create.** Parser JSON-subset → `UnitDoc`.
- `kernel/src/service/unitfile.rs` — **Create.** Builder `UnitDoc`→`Unit`/`Timer`; `schedule_parse`.
- `kernel/src/service/schedule.rs` — **Create.** `Schedule` + `compute_next(Schedule, RtcTime)`.
- `kernel/src/service/topo.rs` — **Create.** `topo_sort` + cycle detection.
- `kernel/src/executor/mod.rs` — **Modify.** `supervisor_task`, `daemon_runner_task` (pool), `scheduler_task`; spawn.
- `kernel/src/wasm/host/service.rs` — **Modify.** Host fn `unit_*`/`timer_*` (TSV), registrazione `link`.
- `kernel/src/boot/phases/userland.rs` — **Modify.** `load_from_disk` + `activate_target(Boot)`; hook `activate_target(PostBoot)` dopo shell/desktop.
- `user/unitctl/` — **Create.** CLI tool (crate wasm32-wasip1).
- `Makefile` — **Modify.** Build + ship `unitctl`.

---

## Phase 1 — Modello dati

### Task 1.1: enums + struct Unit/Timer + registry

**Files:** Modify `kernel/src/service/mod.rs`.

- [ ] **Step 1: leggi il modulo attuale** per non rompere l'API esistente (`Service`, `ServiceInfo`, `register`, `start`, `list`, `status`, `mark_*`, `SERVICE_QUEUE`):
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && sed -n "1,130p" kernel/src/service/mod.rs'
```

- [ ] **Step 2: aggiungi i nuovi tipi** in `service/mod.rs` (dopo gli `use`, prima di `Service`):
```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnitKind { Oneshot, Daemon }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RestartPolicy { No, OnFailure, Always }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActivateTarget { Boot, PostBoot, Manual }

#[derive(Clone)]
pub struct Unit {
    pub name: String,
    pub path: String,
    pub kind: UnitKind,
    pub restart: RestartPolicy,
    pub after: Vec<String>,
    pub requires: Vec<String>,
    pub target: ActivateTarget,
    pub enabled: bool,
    pub status: ServiceStatus,        // riusa l'enum esistente (vedi step 3)
    pub pid: Option<u32>,
    pub runs: u32,
    pub restarts: u32,
    pub stop_requested: bool,
}
```

- [ ] **Step 3: estendi `ServiceStatus`** con `Restarting` (variante nuova). Aggiorna `label()` e ogni `match` su `ServiceStatus` nel file aggiungendo `Restarting => "Restarting"`. Compilerà solo quando tutti i match sono esaustivi.

- [ ] **Step 4: aggiungi registry + queue** in `service/mod.rs`:
```rust
static UNITS: Mutex<Vec<Unit>> = Mutex::new(Vec::new());

/// Richieste runtime cross-core (start/stop/enable/reload) postate dalle host fn,
/// consumate dal manager sul BSP. Pattern di SERVICE_QUEUE.
pub enum UnitRequest {
    Start(String),
    Stop(String),
    Enable(String, bool),
    Reload,
}
pub struct UnitQueue { pub pending: Mutex<VecDeque<UnitRequest>>, pub waker: Mutex<Option<Waker>> }
pub static UNIT_QUEUE: UnitQueue = UnitQueue { pending: Mutex::new(VecDeque::new()), waker: Mutex::new(None) };
```

- [ ] **Step 5: compile** (WSL kernel compile). Expected `Finished`. (Tipi non ancora usati → warning dead_code accettabili in questa fase.)

- [ ] **Step 6: commit**
Crea `CHANGELOG/381-26-06-09-init-unit-model.md` (Cosa: tipi Unit/Timer/enums + UNITS/UNIT_QUEUE; Perché: base init system; File: kernel/src/service/mod.rs). Poi:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add kernel/src/service/mod.rs CHANGELOG/381-26-06-09-init-unit-model.md && git commit -m "feat(init): Unit/Timer model + registries + UNIT_QUEUE"'
```

### Task 1.2: struct Timer + Schedule (in schedule.rs)

**Files:** Create `kernel/src/service/schedule.rs`; Modify `service/mod.rs` (add `pub mod schedule;` + `Timer` struct + `static TIMERS`).

- [ ] **Step 1: crea `schedule.rs`** con l'enum + (compute_next arriva in Fase 3, qui solo il tipo):
```rust
//! Schedule di un timer + calcolo della prossima scadenza.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Schedule {
    EveryTicks(u64),                  // intervallo monotono
    BootPlus(u64),                    // one-shot a boot+N tick
    Hourly { minute: u8 },
    Daily  { hour: u8, minute: u8 },
    Weekly { dow: u8, hour: u8, minute: u8 }, // dow 0=Sun..6=Sat
}
```

- [ ] **Step 2: aggiungi `Timer` + registry** in `service/mod.rs`:
```rust
pub mod schedule;
use schedule::Schedule;

#[derive(Clone)]
pub struct Timer {
    pub name: String,
    pub unit: String,
    pub schedule: Schedule,
    pub enabled: bool,
    pub next_fire: u64,
    pub last_fire: Option<u64>,
}
static TIMERS: Mutex<Vec<Timer>> = Mutex::new(Vec::new());
```

- [ ] **Step 3: compile** (WSL). Expected `Finished`.

- [ ] **Step 4: commit**
Crea `CHANGELOG/382-26-06-09-init-schedule-type.md`. Poi:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add kernel/src/service/schedule.rs kernel/src/service/mod.rs CHANGELOG/382-26-06-09-init-schedule-type.md && git commit -m "feat(init): Schedule enum + Timer registry"'
```

---

## Phase 2 — Parser YAML/JSON + UnitDoc + builder + schedule_parse

### Task 2.1: UnitDoc (modello comune)

**Files:** Create `kernel/src/service/unitdoc.rs`; Modify `service/mod.rs` (`pub mod unitdoc;`).

- [ ] **Step 1: crea `unitdoc.rs`**:
```rust
//! Modello comune prodotto dai parser YAML/JSON e consumato dal builder.
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq)]
pub enum Val { Str(String), Bool(bool), List(Vec<String>) }

#[derive(Clone, Debug, Default)]
pub struct UnitDoc { pub fields: Vec<(String, Val)> }

impl UnitDoc {
    pub fn get(&self, key: &str) -> Option<&Val> {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key) { Some(Val::Str(s)) => Some(s.as_str()), _ => None }
    }
    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.get(key) { Some(Val::Bool(b)) => Some(*b), _ => None }
    }
    pub fn list(&self, key: &str) -> Vec<String> {
        match self.get(key) { Some(Val::List(l)) => l.clone(), _ => Vec::new() }
    }
}
```
Add `pub mod unitdoc;` in `service/mod.rs`.

- [ ] **Step 2: compile** (WSL). Expected `Finished`.

- [ ] **Step 3: commit** — `CHANGELOG/383-26-06-09-init-unitdoc.md`:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add kernel/src/service/unitdoc.rs kernel/src/service/mod.rs CHANGELOG/383-26-06-09-init-unitdoc.md && git commit -m "feat(init): UnitDoc common config model"'
```

### Task 2.2: parser YAML-subset

**Files:** Create `kernel/src/service/yaml.rs`; Modify `service/mod.rs` (`pub mod yaml;`).

- [ ] **Step 1: crea `yaml.rs`** (line-based subset, zero dep):
```rust
//! YAML-subset line-based: `key: value`, liste `[a, b]`, `#` commenti, righe vuote.
//! Nessun nesting. Produce UnitDoc.
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use super::unitdoc::{UnitDoc, Val};

pub fn parse(text: &str) -> Result<UnitDoc, &'static str> {
    let mut doc = UnitDoc::default();
    for raw in text.lines() {
        let line = match raw.split('#').next() { Some(s) => s.trim(), None => "" };
        if line.is_empty() { continue; }
        let (k, v) = line.split_once(':').ok_or("yaml: missing ':'")?;
        let key = k.trim().to_string();
        let val = v.trim();
        let parsed = if val.starts_with('[') && val.ends_with(']') {
            let inner = &val[1..val.len()-1];
            let items: Vec<String> = inner.split(',')
                .map(|s| s.trim().trim_matches('"').to_string())
                .filter(|s| !s.is_empty()).collect();
            Val::List(items)
        } else if val == "true" || val == "false" {
            Val::Bool(val == "true")
        } else {
            Val::Str(val.trim_matches('"').to_string())
        };
        doc.fields.push((key, parsed));
    }
    Ok(doc)
}

#[cfg(feature = "boot-checks")]
pub fn self_test() -> bool {
    let d = parse("name: sshd\ntype: daemon\nenabled: true\nafter: [net, fs]\n# comment\n").unwrap();
    d.str("name") == Some("sshd")
        && d.str("type") == Some("daemon")
        && d.bool("enabled") == Some(true)
        && d.list("after") == alloc::vec!["net".to_string(), "fs".to_string()]
}
```
Add `pub mod yaml;`.

- [ ] **Step 2: compile** (WSL, normale + `--features boot-checks`). Expected `Finished` entrambi.

- [ ] **Step 3: commit** — `CHANGELOG/384-26-06-09-init-yaml-parser.md`:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add kernel/src/service/yaml.rs kernel/src/service/mod.rs CHANGELOG/384-26-06-09-init-yaml-parser.md && git commit -m "feat(init): YAML-subset parser → UnitDoc"'
```

### Task 2.3: parser JSON-subset

**Files:** Create `kernel/src/service/json.rs`; Modify `service/mod.rs` (`pub mod json;`).

- [ ] **Step 1: crea `json.rs`** (subset: oggetto top-level piatto, valori string/bool/array-di-string):
```rust
//! JSON-subset: { "k": "v", "b": true, "l": ["a","b"] }. Oggetto piatto.
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use super::unitdoc::{UnitDoc, Val};

pub fn parse(text: &str) -> Result<UnitDoc, &'static str> {
    let s = text.trim();
    let s = s.strip_prefix('{').ok_or("json: expected '{'")?
             .strip_suffix('}').ok_or("json: expected '}'")?;
    let mut doc = UnitDoc::default();
    let mut chars = s.char_indices().peekable();
    let bytes = s.as_bytes();
    let mut i = 0usize;
    // Parser a stati semplice su `s`.
    fn skip_ws(b: &[u8], i: &mut usize) { while *i < b.len() && (b[*i] as char).is_whitespace() { *i += 1; } }
    fn parse_str(b: &[u8], i: &mut usize) -> Result<String, &'static str> {
        if *i >= b.len() || b[*i] != b'"' { return Err("json: expected string"); }
        *i += 1; let start = *i;
        while *i < b.len() && b[*i] != b'"' { *i += 1; }
        if *i >= b.len() { return Err("json: unterminated string"); }
        let out = core::str::from_utf8(&b[start..*i]).map_err(|_| "json: utf8")?.to_string();
        *i += 1; Ok(out)
    }
    let _ = (chars.peek(), &mut i, bytes);
    let b = s.as_bytes();
    let mut i = 0usize;
    loop {
        skip_ws(b, &mut i);
        if i >= b.len() { break; }
        if b[i] == b',' { i += 1; continue; }
        let key = parse_str(b, &mut i)?;
        skip_ws(b, &mut i);
        if i >= b.len() || b[i] != b':' { return Err("json: expected ':'"); }
        i += 1; skip_ws(b, &mut i);
        if i >= b.len() { return Err("json: value expected"); }
        let val = match b[i] {
            b'"' => Val::Str(parse_str(b, &mut i)?),
            b'[' => {
                i += 1; let mut items = Vec::new();
                loop {
                    skip_ws(b, &mut i);
                    if i < b.len() && b[i] == b']' { i += 1; break; }
                    if i < b.len() && b[i] == b',' { i += 1; continue; }
                    items.push(parse_str(b, &mut i)?);
                }
                Val::List(items)
            }
            b't' | b'f' => {
                let start = i;
                while i < b.len() && (b[i] as char).is_alphabetic() { i += 1; }
                let w = core::str::from_utf8(&b[start..i]).map_err(|_| "json: utf8")?;
                Val::Bool(w == "true")
            }
            _ => return Err("json: unsupported value"),
        };
        doc.fields.push((key, val));
    }
    Ok(doc)
}

#[cfg(feature = "boot-checks")]
pub fn self_test() -> bool {
    let d = parse("{ \"name\": \"sshd\", \"enabled\": true, \"after\": [\"net\",\"fs\"] }").unwrap();
    d.str("name") == Some("sshd") && d.bool("enabled") == Some(true)
        && d.list("after") == alloc::vec!["net".to_string(), "fs".to_string()]
}
```
Add `pub mod json;`. (NB: rimuovi le righe morte `chars`/`char_indices` se il compilatore le segnala — sono residui; tieni solo il parser su `b`/`i`.)

- [ ] **Step 2: compile** (normale + boot-checks). Pulisci eventuali warning di codice morto introdotti.

- [ ] **Step 3: commit** — `CHANGELOG/385-26-06-09-init-json-parser.md`:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add kernel/src/service/json.rs kernel/src/service/mod.rs CHANGELOG/385-26-06-09-init-json-parser.md && git commit -m "feat(init): JSON-subset parser → UnitDoc"'
```

### Task 2.4: schedule_parse + builder UnitDoc→Unit/Timer

**Files:** Create `kernel/src/service/unitfile.rs`; Modify `service/mod.rs` (`pub mod unitfile;`) e `schedule.rs` (aggiungi `parse_schedule`).

- [ ] **Step 1: `schedule_parse`** in `schedule.rs`:
```rust
use alloc::vec::Vec;
/// Parse della stringa schedule. dow: Sun=0..Sat=6.
pub fn parse_schedule(s: &str) -> Result<Schedule, &'static str> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("every ") {
        let secs = rest.trim_end_matches('s').trim().parse::<u64>().map_err(|_| "every: NaN")?;
        return Ok(Schedule::EveryTicks(secs * 100)); // 100 Hz
    }
    if let Some(rest) = s.strip_prefix("boot+") {
        let secs = rest.trim_end_matches('s').trim().parse::<u64>().map_err(|_| "boot+: NaN")?;
        return Ok(Schedule::BootPlus(secs * 100));
    }
    if let Some(rest) = s.strip_prefix("hourly") {
        let min = rest.trim().trim_start_matches(':').trim().parse::<u8>().unwrap_or(0);
        return Ok(Schedule::Hourly { minute: min });
    }
    if let Some(rest) = s.strip_prefix("daily ") {
        let (h, m) = parse_hm(rest.trim())?;
        return Ok(Schedule::Daily { hour: h, minute: m });
    }
    if let Some(rest) = s.strip_prefix("weekly ") {
        let mut it = rest.trim().splitn(2, ' ');
        let dow = parse_dow(it.next().ok_or("weekly: dow")?)?;
        let (h, m) = parse_hm(it.next().ok_or("weekly: time")?)?;
        return Ok(Schedule::Weekly { dow, hour: h, minute: m });
    }
    Err("schedule: unrecognised")
}
fn parse_hm(s: &str) -> Result<(u8, u8), &'static str> {
    let (h, m) = s.split_once(':').ok_or("time: missing ':'")?;
    Ok((h.trim().parse().map_err(|_| "hour")?, m.trim().parse().map_err(|_| "min")?))
}
fn parse_dow(s: &str) -> Result<u8, &'static str> {
    Ok(match s { "Sun"=>0,"Mon"=>1,"Tue"=>2,"Wed"=>3,"Thu"=>4,"Fri"=>5,"Sat"=>6, _=>return Err("dow") })
}

#[cfg(feature = "boot-checks")]
pub fn self_test_parse() -> bool {
    parse_schedule("daily 03:00") == Ok(Schedule::Daily{hour:3,minute:0})
      && parse_schedule("every 300s") == Ok(Schedule::EveryTicks(30000))
      && parse_schedule("weekly Mon 09:30") == Ok(Schedule::Weekly{dow:1,hour:9,minute:30})
      && parse_schedule("hourly :15") == Ok(Schedule::Hourly{minute:15})
}
```

- [ ] **Step 2: builder** in `unitfile.rs`:
```rust
//! Costruisce Unit/Timer da un UnitDoc (format-agnostico). Estensione + `kind` discriminano.
use alloc::string::ToString;
use super::unitdoc::UnitDoc;
use super::schedule::parse_schedule;
use super::{Unit, Timer, UnitKind, RestartPolicy, ActivateTarget, ServiceStatus};

pub enum Built { Unit(Unit), Timer(Timer) }

pub fn build(doc: &UnitDoc) -> Result<Built, &'static str> {
    let name = doc.str("name").ok_or("missing name")?.to_string();
    if doc.str("kind") == Some("timer") {
        let unit = doc.str("unit").ok_or("timer: missing unit")?.to_string();
        let sched = parse_schedule(doc.str("schedule").ok_or("timer: missing schedule")?)?;
        return Ok(Built::Timer(Timer {
            name, unit, schedule: sched,
            enabled: doc.bool("enabled").unwrap_or(false),
            next_fire: 0, last_fire: None,
        }));
    }
    let kind = match doc.str("type") { Some("daemon") => UnitKind::Daemon, _ => UnitKind::Oneshot };
    let restart = match doc.str("restart") {
        Some("always") => RestartPolicy::Always,
        Some("on-failure") => RestartPolicy::OnFailure,
        _ => RestartPolicy::No,
    };
    let target = match doc.str("target") {
        Some("boot") => ActivateTarget::Boot,
        Some("post-boot") => ActivateTarget::PostBoot,
        _ => ActivateTarget::Manual,
    };
    Ok(Built::Unit(Unit {
        name,
        path: doc.str("exec").ok_or("missing exec")?.to_string(),
        kind, restart,
        after: doc.list("after"),
        requires: doc.list("requires"),
        target,
        enabled: doc.bool("enabled").unwrap_or(false),
        status: ServiceStatus::Idle, pid: None, runs: 0, restarts: 0, stop_requested: false,
    }))
}
```
Add `pub mod unitfile;`.

- [ ] **Step 3: compile** (normale + boot-checks).

- [ ] **Step 4: commit** — `CHANGELOG/386-26-06-09-init-unit-builder.md`:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add kernel/src/service/unitfile.rs kernel/src/service/schedule.rs kernel/src/service/mod.rs CHANGELOG/386-26-06-09-init-unit-builder.md && git commit -m "feat(init): schedule_parse + UnitDoc→Unit/Timer builder"'
```

---

## Phase 3 — compute_next (calendario)

### Task 3.1: compute_next + boot-check

**Files:** Modify `kernel/src/service/schedule.rs`.

- [ ] **Step 1: implementa `compute_next`** in `schedule.rs`. Usa `crate::rtc::{RtcTime, to_unix_epoch}` (RtcTime ha `year:u16,month:u8,day:u8,hour:u8,minute:u8,second:u8`). Riusa `to_unix_epoch` per il risultato epoch; per il calcolo "prossima occorrenza" costruisci l'`RtcTime` target e, se non futuro, avanza di 1 ora/giorno/settimana ricostruendo via aritmetica su epoch (più semplice che gestire i rollover a mano):
```rust
use crate::rtc::{RtcTime, to_unix_epoch, from_unix_epoch}; // from_unix_epoch: vedi step 2

/// Prossima scadenza FUTURA. Calendario → epoch (s). Interval/BootPlus → tick.
pub fn compute_next(s: Schedule, now: RtcTime, now_ticks: u64) -> u64 {
    match s {
        Schedule::EveryTicks(n) => now_ticks + n,
        Schedule::BootPlus(n)   => now_ticks + n, // armato una volta (vedi scheduler)
        Schedule::Hourly { minute } => {
            let mut e = to_unix_epoch(&RtcTime { minute, second: 0, ..now });
            let cur = to_unix_epoch(&now);
            if e <= cur { e += 3600; }
            e
        }
        Schedule::Daily { hour, minute } => {
            let mut e = to_unix_epoch(&RtcTime { hour, minute, second: 0, ..now });
            let cur = to_unix_epoch(&now);
            if e <= cur { e += 86400; }
            e
        }
        Schedule::Weekly { dow, hour, minute } => {
            let cur = to_unix_epoch(&now);
            let mut e = to_unix_epoch(&RtcTime { hour, minute, second: 0, ..now });
            // 1970-01-01 era giovedì (dow 4). dow del giorno corrente:
            let day_dow = (((cur / 86400) + 4) % 7) as u8; // 0=Sun
            let mut delta = (7 + dow as i64 - day_dow as i64) % 7;
            if delta == 0 && e <= cur { delta = 7; }
            e += (delta as u64) * 86400;
            if delta == 0 && e <= cur { e += 7 * 86400; } // safety
            e
        }
    }
}
```
Nota: `..now` (struct update) richiede che `RtcTime` derivi `Clone`/`Copy`. Se non lo deriva, costruisci il campo a mano copiando i campi rimanenti da `now`. Verifica leggendo `rtc.rs`.

- [ ] **Step 2: aggiungi `from_unix_epoch`** in `kernel/src/rtc.rs` SE non esiste (serve solo se preferisci ricostruire da epoch; con l'approccio "costruisci RtcTime + bump epoch" sopra NON serve — in tal caso salta questo step). Verifica: `grep -n "from_unix_epoch" kernel/src/rtc.rs`. Se manca e non lo usi, rimuovi l'`use` di `from_unix_epoch`.

- [ ] **Step 3: boot-check** in `schedule.rs`:
```rust
#[cfg(feature = "boot-checks")]
pub fn self_test_compute() -> bool {
    use crate::rtc::{RtcTime, to_unix_epoch};
    let now = RtcTime { year: 2026, month: 6, day: 9, hour: 14, minute: 30, second: 0 };
    let daily = compute_next(Schedule::Daily{hour:3,minute:0}, now, 0);
    let want_daily = to_unix_epoch(&RtcTime{ year:2026,month:6,day:10,hour:3,minute:0,second:0 });
    let hourly = compute_next(Schedule::Hourly{minute:0}, now, 0);
    let want_hourly = to_unix_epoch(&RtcTime{ year:2026,month:6,day:9,hour:15,minute:0,second:0 });
    daily == want_daily && hourly == want_hourly
}
```

- [ ] **Step 4: compile** (normale + boot-checks). Fix `RtcTime` Copy/struct-update se serve.

- [ ] **Step 5: commit** — `CHANGELOG/387-26-06-09-init-compute-next.md`:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add kernel/src/service/schedule.rs kernel/src/rtc.rs CHANGELOG/387-26-06-09-init-compute-next.md && git commit -m "feat(init): compute_next calendar scheduling"'
```

---

## Phase 4 — Supervisor (daemon runner + restart + stop)

### Task 4.1: API manager (start/stop/enable) + mark_* estese

**Files:** Modify `kernel/src/service/mod.rs`.

- [ ] **Step 1: aggiungi le API unit** in `service/mod.rs` (affianco alle `service::*` esistenti — NON rimuovere le vecchie finché la Fase 8 non migra i chiamanti):
```rust
pub fn units_list() -> Vec<Unit> { UNITS.lock().clone() }
pub fn unit_get(name: &str) -> Option<Unit> { UNITS.lock().iter().find(|u| u.name == name).cloned() }
pub fn timers_list() -> Vec<Timer> { TIMERS.lock().clone() }

pub fn unit_register(u: Unit) {
    let mut r = UNITS.lock();
    if r.iter().any(|x| x.name == u.name) { return; }
    r.push(u);
}
pub fn timer_register(t: Timer) {
    let mut r = TIMERS.lock();
    if r.iter().any(|x| x.name == t.name) { return; }
    r.push(t);
}

/// Posta una richiesta runtime (cross-core safe). Sveglia il manager BSP.
pub fn post(req: UnitRequest) {
    UNIT_QUEUE.pending.lock().push_back(req);
    if let Some(w) = UNIT_QUEUE.waker.lock().take() { w.wake(); }
}

// Mutazioni di stato (chiamate dai runner/dispatcher sul BSP):
pub fn unit_mark_running(name: &str, pid: u32) { with_unit(name, |u| { u.status = ServiceStatus::Running; u.pid = Some(pid); u.runs = u.runs.saturating_add(1); }); }
pub fn unit_mark_exited(name: &str, code: i32) { with_unit(name, |u| { u.status = ServiceStatus::Exited(code); u.pid = None; }); }
pub fn unit_mark_failed(name: &str, why: &'static str) { with_unit(name, |u| { u.status = ServiceStatus::Failed(why); u.pid = None; }); }
pub fn unit_mark_restarting(name: &str) { with_unit(name, |u| { u.status = ServiceStatus::Restarting; u.pid = None; u.restarts = u.restarts.saturating_add(1); }); }
pub fn unit_set_stop(name: &str) { with_unit(name, |u| u.stop_requested = true); }
fn with_unit(name: &str, f: impl FnOnce(&mut Unit)) { if let Some(u) = UNITS.lock().iter_mut().find(|u| u.name == name) { f(u); } }
```

- [ ] **Step 2: compile** (WSL). Expected `Finished`.

- [ ] **Step 3: commit** — `CHANGELOG/388-26-06-09-init-manager-api.md`:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add kernel/src/service/mod.rs CHANGELOG/388-26-06-09-init-manager-api.md && git commit -m "feat(init): unit manager API (register/post/mark_*)"'
```

### Task 4.2: backoff + daemon runner + manager task

**Files:** Modify `kernel/src/executor/mod.rs`; Modify `service/mod.rs` (backoff helper).

- [ ] **Step 1: leggi i pattern esistenti** da rispecchiare:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && sed -n "771,815p" kernel/src/executor/mod.rs && grep -n "ssh_pty_dispatcher_task\|pool_size\|spawn(" kernel/src/executor/mod.rs | head'
```
Studia `service_dispatcher_task` (oneshot inline) e `ssh_pty_dispatcher_task` (pool tasks) + come `exec_cwasm_parallel` viene chiamato per `.cwasm`.

- [ ] **Step 2: `backoff`** (puro) in `service/mod.rs`:
```rust
/// Backoff esponenziale capato a 30s, in tick (100 Hz). 1,2,4,…,30s.
pub fn backoff_ticks(restarts: u32) -> u64 {
    let secs = core::cmp::min(1u64 << restarts.min(5), 30);
    secs * 100
}
#[cfg(feature = "boot-checks")]
pub fn self_test_backoff() -> bool {
    backoff_ticks(0)==100 && backoff_ticks(1)==200 && backoff_ticks(2)==400 && backoff_ticks(10)==3000
}
```

- [ ] **Step 3: manager + runner task** in `executor/mod.rs`. Aggiungi un `manager_task` (consuma `UNIT_QUEUE`: Start→instrada oneshot al dispatcher esistente / daemon a un runner libero; Stop→`request_kill`+`unit_set_stop`; Enable→`service::set_enabled`+persist (Fase 7); Reload→`service::load_from_disk`) e un pool di `daemon_runner_task` (`#[embassy_executor::task(pool_size = 8)]`) che esegue il loop daemon della spec §3:
```rust
#[embassy_executor::task(pool_size = 8)]
async fn daemon_runner_task(name: alloc::string::String) {
    use crate::service::*;
    loop {
        let u = match unit_get(&name) { Some(u) => u, None => break };
        let pid = crate::proc::register(name.clone());
        unit_mark_running(&name, pid);
        let code = if u.path.ends_with(".cwasm") {
            crate::wasm::fiber::exec_cwasm_parallel(&u.path, alloc::vec![name.as_bytes().to_vec()], 0).await
        } else {
            run_unit_wasmi(&u.path, &name).await   // helper attorno al path wasmi del dispatcher esistente
        };
        crate::proc::unregister(pid);
        if code == 0 { unit_mark_exited(&name, code); } else { unit_mark_failed(&name, "exit"); }
        let u = match unit_get(&name) { Some(u) => u, None => break };
        if u.stop_requested { break; }
        let restart = matches!(u.restart, RestartPolicy::Always)
            || (matches!(u.restart, RestartPolicy::OnFailure) && code != 0);
        if !restart { break; }
        unit_mark_restarting(&name);
        crate::executor::delay::Delay::ticks(backoff_ticks(u.restarts)).await;
    }
}
```
`run_unit_wasmi(path, name)`: estrai il path wasmi dell'attuale `service_dispatcher_task` (read+instantiate+`fb.run().await`) in un helper riusabile (refactor di quel task) così oneshot e daemon-wasm lo condividono. Lo spawn di un runner: serve uno `Spawner` accessibile — segui come `ssh_pty_dispatcher_task` ottiene/usa lo spawner (probabilmente un canale di spawn o `Spawner` statico). Se lo spawn dinamico non è banale, instrada i daemon a un set fisso di runner via una coda `DAEMON_QUEUE` (come `PTY_QUEUE`/`enqueue_shell_pty`) consumata da `pool_size` task `daemon_runner_task` che fanno `WaitForDaemon.await` del nome. **Mirror del pattern PTY.**

- [ ] **Step 4: spawna manager + pool** dove gli altri task sono spawnati (~riga 256-258): `spawner.spawn(manager_task()).unwrap();` + i runner del pool come fa il PTY dispatcher.

- [ ] **Step 5: compile** (normale + boot-checks). Risolvi l'API spawn (è il punto più delicato — vedi Step 3).

- [ ] **Step 6: commit** — `CHANGELOG/389-26-06-09-init-supervisor.md`:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add kernel/src/executor/mod.rs kernel/src/service/mod.rs CHANGELOG/389-26-06-09-init-supervisor.md && git commit -m "feat(init): supervisor — daemon runner pool + restart/backoff + cooperative stop"'
```

---

## Phase 5 — Scheduler

### Task 5.1: scheduler_task (polling 1s)

**Files:** Modify `kernel/src/executor/mod.rs`.

- [ ] **Step 1: scheduler_task** (pattern dei task esistenti):
```rust
#[embassy_executor::task]
async fn scheduler_task() {
    let _ = crate::proc::register_kernel("scheduler");
    use crate::service::{timers_list, post, UnitRequest, schedule::{Schedule, compute_next}};
    loop {
        crate::executor::delay::Delay::ticks(100).await; // ~1s
        let ticks = crate::timer::ticks();
        let now = crate::rtc::now();
        let epoch = crate::rtc::to_unix_epoch(&now);
        for t in timers_list() {
            if !t.enabled { continue; }
            let due = match t.schedule {
                Schedule::EveryTicks(_) | Schedule::BootPlus(_) => ticks >= t.next_fire,
                _ => epoch >= t.next_fire,
            };
            if due {
                post(UnitRequest::Start(t.unit.clone()));
                crate::service::timer_fired(&t.name, ticks, &now); // aggiorna last/next, disarma BootPlus
            }
        }
    }
}
```

- [ ] **Step 2: `timer_fired`** in `service/mod.rs`:
```rust
pub fn timer_fired(name: &str, now_ticks: u64, now: &crate::rtc::RtcTime) {
    let mut r = TIMERS.lock();
    if let Some(t) = r.iter_mut().find(|t| t.name == name) {
        let stamp = match t.schedule {
            schedule::Schedule::EveryTicks(_) | schedule::Schedule::BootPlus(_) => now_ticks,
            _ => crate::rtc::to_unix_epoch(now),
        };
        t.last_fire = Some(stamp);
        if matches!(t.schedule, schedule::Schedule::BootPlus(_)) { t.enabled = false; }
        else { t.next_fire = schedule::compute_next(t.schedule, *now, now_ticks); }
    }
}
```

- [ ] **Step 3: spawna `scheduler_task`** accanto al manager (Fase 4 step 4).

- [ ] **Step 4: compile** (normale + boot-checks). Expected `Finished`.

- [ ] **Step 5: commit** — `CHANGELOG/390-26-06-09-init-scheduler.md`:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add kernel/src/executor/mod.rs kernel/src/service/mod.rs CHANGELOG/390-26-06-09-init-scheduler.md && git commit -m "feat(init): scheduler task (1s polling, interval + calendar timers)"'
```

---

## Phase 6 — Dipendenze + target + attivazione

### Task 6.1: topo_sort

**Files:** Create `kernel/src/service/topo.rs`; Modify `service/mod.rs` (`pub mod topo;`).

- [ ] **Step 1: `topo.rs`** (Kahn + ciclo):
```rust
//! Topo-sort di nomi unit su archi `after ∪ requires`. Ritorna l'ordine, oppure
//! l'insieme dei nodi in ciclo.
use alloc::string::String;
use alloc::vec::Vec;

/// `edges`: (unit, dep) significa "unit DOPO dep". Ritorna Ok(ordine) o Err(ciclo).
pub fn topo_sort(nodes: &[String], edges: &[(String, String)]) -> Result<Vec<String>, Vec<String>> {
    let mut indeg: Vec<(String, usize)> = nodes.iter().map(|n| (n.clone(), 0)).collect();
    for (u, _dep) in edges {
        if let Some(e) = indeg.iter_mut().find(|(n, _)| n == u) { e.1 += 1; }
    }
    let mut order = Vec::new();
    let mut queue: Vec<String> = indeg.iter().filter(|(_, d)| *d == 0).map(|(n, _)| n.clone()).collect();
    let mut done: Vec<String> = Vec::new();
    while let Some(n) = queue.pop() {
        order.push(n.clone()); done.push(n.clone());
        for (u, dep) in edges {
            if *dep == n {
                if let Some(e) = indeg.iter_mut().find(|(x, _)| x == u) {
                    e.1 -= 1;
                    if e.1 == 0 { queue.push(u.clone()); }
                }
            }
        }
    }
    if order.len() == nodes.len() { Ok(order) }
    else { Err(nodes.iter().filter(|n| !done.contains(n)).cloned().collect()) }
}

#[cfg(feature = "boot-checks")]
pub fn self_test() -> bool {
    let nodes = alloc::vec!["a".into(), "b".into()];
    let edges = alloc::vec![("a".to_string(), "b".to_string())]; // a after b
    let ok = topo_sort(&nodes, &edges) == Ok(alloc::vec!["b".to_string(), "a".to_string()]);
    let cyc_nodes = alloc::vec!["x".into(), "y".into()];
    let cyc_edges = alloc::vec![("x".into(),"y".into()), ("y".into(),"x".into())];
    let cyc = topo_sort(&cyc_nodes, &cyc_edges).is_err();
    ok && cyc
}
```
Add `pub mod topo;`.

- [ ] **Step 2: compile** (normale + boot-checks). NB: l'ordine esatto del topo dipende dall'ordine di pop; il test usa un grafo a 2 nodi con risultato deterministico. Se l'ordine differisce per via di `pop()` (LIFO), adatta l'assert al risultato reale (entrambi gli ordini validi rispettano la dipendenza — l'importante è b prima di a).

- [ ] **Step 3: commit** — `CHANGELOG/391-26-06-09-init-topo-sort.md`:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add kernel/src/service/topo.rs kernel/src/service/mod.rs CHANGELOG/391-26-06-09-init-topo-sort.md && git commit -m "feat(init): topo_sort + cycle detection"'
```

### Task 6.2: activate_target

**Files:** Modify `kernel/src/service/mod.rs`.

- [ ] **Step 1: `activate_target`** in `service/mod.rs`:
```rust
/// Attiva tutte le unit `enabled` del target dato, in ordine topologico
/// (after ∪ requires), tirando su i `requires`. Chiamata sul BSP.
pub async fn activate_target(target: ActivateTarget) {
    let units = units_list();
    // set = enabled+target + chiusura requires
    let mut want: Vec<String> = units.iter()
        .filter(|u| u.enabled && u.target == target)
        .map(|u| u.name.clone()).collect();
    let mut i = 0;
    while i < want.len() {
        let reqs = units.iter().find(|u| u.name == want[i]).map(|u| u.requires.clone()).unwrap_or_default();
        for r in reqs { if !want.contains(&r) { want.push(r); } }
        i += 1;
    }
    // archi
    let mut edges = Vec::new();
    for u in units.iter().filter(|u| want.contains(&u.name)) {
        for d in u.after.iter().chain(u.requires.iter()) {
            if want.contains(d) { edges.push((u.name.clone(), d.clone())); }
        }
    }
    let order = match topo::topo_sort(&want, &edges) {
        Ok(o) => o,
        Err(cycle) => { for n in &cycle { unit_mark_failed(n, "cycle"); }
            crate::bwarn!("init", "dependency cycle: {:?}", cycle);
            want.iter().filter(|n| !cycle.contains(n)).cloned().collect() }
    };
    for name in order {
        post(UnitRequest::Start(name.clone()));
        // attende "su" con timeout ~10s (daemon Running / oneshot Exited)
        let deadline = crate::timer::ticks() + 1000;
        loop {
            if let Some(u) = unit_get(&name) {
                if matches!(u.status, ServiceStatus::Running | ServiceStatus::Exited(_)) { break; }
                if matches!(u.status, ServiceStatus::Failed(_)) { break; }
            }
            if crate::timer::ticks() > deadline { crate::bwarn!("init", "activate timeout: {}", name); break; }
            crate::executor::delay::Delay::ticks(5).await;
        }
    }
}
```
(NB: `post` instrada via manager; in alternativa chiama una `start_unit(name)` sincrona che instrada direttamente — scegli in base a come hai fatto il manager in Fase 4. Mantieni coerenza.)

- [ ] **Step 2: compile** (WSL). Expected `Finished`.

- [ ] **Step 3: commit** — `CHANGELOG/392-26-06-09-init-activate-target.md`:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add kernel/src/service/mod.rs CHANGELOG/392-26-06-09-init-activate-target.md && git commit -m "feat(init): activate_target with topo ordering + requires + timeout"'
```

---

## Phase 7 — load_from_disk + persistenza + boot hooks

### Task 7.1: load_from_disk

**Files:** Modify `kernel/src/service/mod.rs`; verifica API VFS readdir.

- [ ] **Step 1: trova l'API VFS** per elencare una dir + leggere un file (kernel-side, NON le host fn wasi):
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && grep -rn "pub fn read_dir\|pub fn readdir\|pub fn read(\|fn open(\|list_dir\|pub fn stat" kernel/src/vfs/mod.rs | head'
```

- [ ] **Step 2: `load_from_disk`** in `service/mod.rs` (adatta ai nomi VFS reali trovati):
```rust
/// Legge /mnt/etc/units/*.{yaml,json}, parsa, registra. Robusto: file rotto → log+skip.
pub fn load_from_disk() {
    const DIR: &str = "/mnt/etc/units";
    let entries = match crate::vfs::read_dir(DIR) { Ok(e) => e, Err(_) => { crate::binfo!("init", "no {} (builtin only)", DIR); return; } };
    for name in entries {
        let is_yaml = name.ends_with(".yaml");
        let is_json = name.ends_with(".json");
        if !is_yaml && !is_json { continue; }
        let path = alloc::format!("{}/{}", DIR, name);
        let bytes = match crate::vfs::read_file(&path) { Ok(b) => b, Err(_) => { crate::bwarn!("init", "read {}", path); continue; } };
        let text = match core::str::from_utf8(&bytes) { Ok(t) => t, Err(_) => { crate::bwarn!("init", "utf8 {}", path); continue; } };
        let doc = match if is_yaml { yaml::parse(text) } else { json::parse(text) } {
            Ok(d) => d, Err(e) => { crate::bwarn!("init", "parse {}: {}", path, e); continue; }
        };
        match unitfile::build(&doc) {
            Ok(unitfile::Built::Unit(u)) => unit_register(u),
            Ok(unitfile::Built::Timer(mut t)) => {
                let now = crate::rtc::now();
                t.next_fire = schedule::compute_next(t.schedule, now, crate::timer::ticks());
                timer_register(t);
            }
            Err(e) => crate::bwarn!("init", "build {}: {}", path, e),
        }
    }
}
```
Se l'API VFS non offre `read_dir`/`read_file` con queste firme, scrivi piccoli wrapper attorno a ciò che esiste (open+fd_readdir+read) — mantieni `load_from_disk` come unico punto.

- [ ] **Step 3: `set_enabled` + persistenza** in `service/mod.rs`:
```rust
/// enable/disable: aggiorna registry + riscrive il file della unit.
pub fn set_enabled(name: &str, on: bool) -> Result<(), ServiceError> {
    with_unit(name, |u| u.enabled = on);
    persist_unit(name)  // riscrive /mnt/etc/units/<name>.yaml in formato canonico
}
fn persist_unit(name: &str) -> Result<(), ServiceError> {
    let u = unit_get(name).ok_or(ServiceError::NotFound)?;
    let mut s = String::new();
    use core::fmt::Write as _;
    let _ = writeln!(s, "name: {}", u.name);
    let _ = writeln!(s, "type: {}", match u.kind { UnitKind::Daemon=>"daemon", _=>"oneshot" });
    let _ = writeln!(s, "exec: {}", u.path);
    let _ = writeln!(s, "restart: {}", match u.restart { RestartPolicy::Always=>"always", RestartPolicy::OnFailure=>"on-failure", _=>"no" });
    let _ = writeln!(s, "target: {}", match u.target { ActivateTarget::Boot=>"boot", ActivateTarget::PostBoot=>"post-boot", _=>"manual" });
    let _ = writeln!(s, "enabled: {}", u.enabled);
    if !u.after.is_empty() { let _ = writeln!(s, "after: [{}]", u.after.join(", ")); }
    if !u.requires.is_empty() { let _ = writeln!(s, "requires: [{}]", u.requires.join(", ")); }
    crate::vfs::write_file(&alloc::format!("/mnt/etc/units/{}.yaml", name), s.as_bytes())
        .map_err(|_| ServiceError::Internal)
}
```
Aggiungi varianti `NoSlot`/`Parse` a `ServiceError` + `errno()`.

- [ ] **Step 4: compile** (WSL). Expected `Finished`.

- [ ] **Step 5: commit** — `CHANGELOG/393-26-06-09-init-load-persist.md`:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add kernel/src/service/mod.rs CHANGELOG/393-26-06-09-init-load-persist.md && git commit -m "feat(init): load_from_disk + enable/disable persistence"'
```

### Task 7.2: boot hooks

**Files:** Modify `kernel/src/boot/phases/userland.rs`.

- [ ] **Step 1: trova i punti** init service + spawn shell/desktop:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && grep -n "service::init\|ssh::spawn\|compositor\|gui\|shell" kernel/src/boot/phases/userland.rs'
```

- [ ] **Step 2: cabla i hook**: dopo `service::init()` e dopo che storage+net sono su, aggiungi:
```rust
    crate::service::load_from_disk();
    crate::service::activate_target(crate::service::ActivateTarget::Boot).await;
```
e DOPO lo spawn di shell/desktop (fine fase), aggiungi:
```rust
    crate::service::activate_target(crate::service::ActivateTarget::PostBoot).await;
```
(Se la fase non è `async` nel punto richiesto, posta un `UnitRequest`/usa un flag che il manager_task processa appena attivo — adatta al control-flow reale del boot. L'importante: Boot dopo storage/net, PostBoot dopo shell.)

- [ ] **Step 3: compile** (WSL). Expected `Finished`.

- [ ] **Step 4: commit** — `CHANGELOG/394-26-06-09-init-boot-hooks.md`:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add kernel/src/boot/phases/userland.rs CHANGELOG/394-26-06-09-init-boot-hooks.md && git commit -m "feat(init): boot hooks — load_from_disk + activate Boot/PostBoot"'
```

---

## Phase 8 — Host ABI + tool unitctl

### Task 8.1: host fn unit_*/timer_*

**Files:** Modify `kernel/src/wasm/host/service.rs`.

- [ ] **Step 1: estendi `service.rs`** mantenendo le `ruos_service_*` esistenti, aggiungendo (stesso stile TSV + guest_read/guest_write):
```rust
// TSV unit: name<TAB>status<TAB>pid<TAB>runs<TAB>restarts<TAB>kind<TAB>target<TAB>path\n
// TSV timer: name<TAB>unit<TAB>schedule<TAB>next_fire<TAB>last_fire\n
```
Implementa `ruos_unit_list`, `ruos_unit_status`, `ruos_unit_start` (`service::post(UnitRequest::Start(..))`→0), `ruos_unit_stop` (`post(Stop)`), `ruos_unit_enable(name,on)` (`post(Enable)`), `ruos_timer_list`, `ruos_unit_reload` (`post(Reload)`). Riusa `read_name`, `guest_write`, `guest_write_u32` esistenti. Per `kind`/`target`/`schedule` aggiungi formatter `&str` (in `service/mod.rs`: `UnitKind::as_str`, ecc.).

- [ ] **Step 2: registra in `link`**:
```rust
        .func_wrap("ruos", "unit_list",   ruos_unit_list)?
        .func_wrap("ruos", "unit_status", ruos_unit_status)?
        .func_wrap("ruos", "unit_start",  ruos_unit_start)?
        .func_wrap("ruos", "unit_stop",   ruos_unit_stop)?
        .func_wrap("ruos", "unit_enable", ruos_unit_enable)?
        .func_wrap("ruos", "timer_list",  ruos_timer_list)?
        .func_wrap("ruos", "unit_reload", ruos_unit_reload)?;
```

- [ ] **Step 3: compile** (WSL). Expected `Finished`.

- [ ] **Step 4: commit** — `CHANGELOG/395-26-06-09-init-host-abi.md`:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add kernel/src/wasm/host/service.rs kernel/src/service/mod.rs CHANGELOG/395-26-06-09-init-host-abi.md && git commit -m "feat(init): host ABI unit_*/timer_* (TSV)"'
```

### Task 8.2: tool unitctl

**Files:** Create `user/unitctl/{Cargo.toml,src/main.rs}`; Modify `Makefile`, `user/Cargo.toml` (workspace member).

- [ ] **Step 1: copia un tool esistente come scheletro**:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && ls user/ && grep -rn "service_list\|service_start\|extern" user/service/src/main.rs 2>/dev/null | head; cat user/which/Cargo.toml'
```
Replica il pattern del tool `service` (o `which`) per gli `extern "C"` verso `ruos.*` + `wasm32-wasip1`.

- [ ] **Step 2: `user/unitctl/Cargo.toml`** (mirror di un tool esistente, stesso edition/deps/profile).

- [ ] **Step 3: `user/unitctl/src/main.rs`**: dichiara gli extern `ruos.unit_list/unit_status/unit_start/unit_stop/unit_enable/timer_list/unit_reload`, parse `argv` per i subcomandi (`list/status/start/stop/enable/disable/timers/reload/cat`), chiama la host fn, stampa il TSV formattato a colonne. `cat <name>` legge il file `/mnt/etc/units/<name>.{yaml,json}` via WASI std fs e lo stampa.

- [ ] **Step 4: workspace + Makefile**: aggiungi `unitctl` ai membri di `user/Cargo.toml` se serve; aggiungi al Makefile la build+copia (mirror della regola `user-wasm`/`user/%`); assicurati che `unitctl.wasm` finisca in `user-bin/` e sull'ISO (`/bin`).

- [ ] **Step 5: build il tool** (WSL): `make user/unitctl` (o l'equivalente trovato). Expected: `unitctl.wasm` prodotto.

- [ ] **Step 6: commit** — `CHANGELOG/396-26-06-09-init-unitctl-tool.md`:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add user/unitctl Makefile user/Cargo.toml CHANGELOG/396-26-06-09-init-unitctl-tool.md && git commit -m "feat(init): unitctl userspace CLI"'
```

---

## Phase 9 — Boot-checks + verifica end-to-end

### Task 9.1: boot-check runner

**Files:** Modify `kernel/src/wasm/wt/mod.rs` o il sito boot-checks; Modify `service/mod.rs` (aggregatore).

- [ ] **Step 1: trova il sito boot-checks** che invoca i self-test e asserisce:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && grep -rn "self_test\|run_.*_demo\|boot-checks" kernel/src/boot kernel/src/main.rs | head'
```

- [ ] **Step 2: aggregatore** in `service/mod.rs`:
```rust
#[cfg(feature = "boot-checks")]
pub fn self_test_all() -> bool {
    yaml::self_test() && json::self_test()
        && schedule::self_test_parse() && schedule::self_test_compute()
        && topo::self_test() && self_test_backoff()
}
```

- [ ] **Step 3: invoca + asserisci** al sito boot-checks: chiama `crate::service::self_test_all()`, logga `INIT-SELFTEST-OK` se true (formato coerente con gli altri marker → `make run-test` lo vede), panica/loga FAIL altrimenti.

- [ ] **Step 4: build boot-checks + run-test**:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/w/Work/GitHub/ruos && make iso CARGO_FEATURES=boot-checks 2>&1 | tail -15 && make run-test 2>&1 | tail -30'
```
Expected: boot completa, `INIT-SELFTEST-OK` nel log, `make run-test` asserisce successo (no panic).

- [ ] **Step 5: commit** — `CHANGELOG/397-26-06-09-init-bootchecks.md`:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add kernel/src/service/mod.rs <sito-boot-checks> CHANGELOG/397-26-06-09-init-bootchecks.md && git commit -m "test(init): boot-checks (parsers, compute_next, topo, backoff)"'
```

### Task 9.2: ISO completa + verifica manuale

**Files:** nessun codice; integrazione + verifica.

- [ ] **Step 1: ISO completa**:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/w/Work/GitHub/ruos && make iso 2>&1 | tail -15'
```
Expected: ISO assemblata, `unitctl.wasm` su `/bin`.

- [ ] **Step 2: prepara unit di test** su `/mnt/etc/units/` (via il sistema bootato o pre-popolando l'immagine FAT32): un `hello.yaml` (oneshot, target manual, exec un tool esistente), un `tick.yaml` (timer `every 10s` → hello), un daemon `.cwasm` con `restart: on-failure`.

- [ ] **Step 3: verifica manuale** (`make run` / VBox):
  1. `unitctl list` mostra le unit dai file (yaml+json).
  2. `unitctl start hello` → gira una volta, status `Exited(0)`.
  3. timer `every 10s` → `hello` riparte ogni 10s (`unitctl timers` mostra `next_fire`).
  4. daemon → `Running`; killalo (fallo crashare) → restart per policy; `unitctl stop <d>` → resta giù.
  5. `unitctl enable/disable hello` → persiste; reboot → stato conservato.
  6. unit con `after`/`requires` → ordine rispettato; `requires` fallito → dipendente `Failed(dep)`.
  7. `target: post-boot` → attiva dopo shell; `boot` → durante boot.
  Se un punto fallisce → debug (netconsole per i log) prima di chiudere.

- [ ] **Step 4: changelog finale + (se richiesto) push**
Crea `CHANGELOG/398-26-06-09-init-system-integration.md`. Commit:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && git add CHANGELOG/398-26-06-09-init-system-integration.md && git commit -m "feat(init): native init system (units+timers+supervision) integration"'
```
Push solo su richiesta esplicita, via WSL interattivo.

---

## Note finali

- **Punto più delicato = Fase 4 (spawn dei daemon runner).** L'API embassy per spawnare task dinamici è statica (pool_size). Usa il pattern PTY: una `DAEMON_QUEUE` + `pool_size` task `daemon_runner_task` che fanno `WaitForDaemon.await`. NON inventare spawn dinamico se il codebase non lo offre — rispecchia `ssh_pty_dispatcher_task`.
- **VFS write** (Fase 7 persist): verifica che il VFS kernel-side esponga scrittura su FAT32 `/mnt`. Se è read-only, la persistenza enable/disable va rivista (degrada a in-RAM + warn) — verifica e adatta, documentando.
- **Refactor del dispatcher esistente** (Fase 4 `run_unit_wasmi`): estrai il path wasmi da `service_dispatcher_task` in un helper condiviso; mantieni il vecchio `service::start`/host fn `ruos_service_*` finché `unitctl` non sostituisce il tool `service` (rimozione opzionale a fine progetto).
- **No `cargo test` kernel**: copertura = funzioni pure (parser/compute_next/topo/backoff/schedule_parse) via boot-checks + manuale end-to-end.
- **shared-nothing**: tutte le mutazioni di registry sul BSP; host fn da altri core solo `post()` in `UNIT_QUEUE`. Letture con lock minimo.
